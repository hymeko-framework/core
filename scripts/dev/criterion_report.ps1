<#
.SYNOPSIS
    Extract Criterion results from target/criterion into a comparable table.

.DESCRIPTION
    Criterion's console output is transient and easy to lose to a pipe; the
    authoritative data is the JSON it writes under
    `target/criterion/<group>/<function>/<value>/<baseline>/`. This script reads
    that, so a table can be regenerated at any time without re-running a
    benchmark (CLAUDE.md: "do not remeasure from scratch when a relevant,
    trustworthy artifact already exists").

    Reports **median** and the Criterion confidence interval, per Sec. 3
    ("single-shot wall-clock numbers are not measurements"), plus throughput
    where the benchmark declared one.

.PARAMETER Baseline
    Baseline directory name to read (e.g. before, after, t1, t24).

.PARAMETER Compare
    Optional second baseline. When given, the table gains a delta column
    (negative = the Compare baseline is faster) and a flag for anything past
    the Sec. 3 10 % regression gate.

.PARAMETER Filter
    Optional regex on the benchmark id.

.PARAMETER Csv
    Optional path to also write the table as CSV.

.EXAMPLE
    .\criterion_report.ps1 -Baseline before

.EXAMPLE
    .\criterion_report.ps1 -Baseline before -Compare after -Csv results.csv
#>
[CmdletBinding()]
param(
    [string]$Baseline = 'before',
    [string]$Compare = '',
    [string]$Filter = '',
    [string]$Csv = '',
    [string]$CriterionRoot = '',
    [switch]$PassThru
)

$ErrorActionPreference = 'Stop'
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$critRoot = if ($CriterionRoot) { [IO.Path]::GetFullPath($CriterionRoot) } else { Join-Path $repoRoot 'target\criterion' }
if (-not (Test-Path $critRoot)) { throw "No Criterion data at $critRoot - run cargo bench first." }

function Get-Estimates {
    param([string]$BaselineName)

    $rows = @{}
    Get-ChildItem -Path $critRoot -Recurse -Filter 'estimates.json' -ErrorAction SilentlyContinue |
        Where-Object { $_.Directory.Name -eq $BaselineName } |
        ForEach-Object {
            $dir = $_.Directory
            $est = Get-Content $_.FullName -Raw | ConvertFrom-Json

            # benchmark.json carries the canonical id and the declared throughput.
            $benchJson = Join-Path $dir.FullName 'benchmark.json'
            $throughputKind = 'elements'
            if (Test-Path $benchJson) {
                $meta = Get-Content $benchJson -Raw | ConvertFrom-Json
                $id = $meta.full_id
                $elems = $null
                if ($meta.throughput) {
                    if ($meta.throughput.Elements)   { $elems = [double]$meta.throughput.Elements }
                    elseif ($meta.throughput.Bytes)  {
                        $elems = [double]$meta.throughput.Bytes
                        $throughputKind = 'bytes'
                    }
                }
            } else {
                # Fall back to the directory path relative to target/criterion.
                $id = $dir.Parent.FullName.Substring($critRoot.Length).TrimStart('\') -replace '\\', '/'
                $elems = $null
            }

            # Samples are iteration-batch means, not individual request latencies.
            $samplePath = Join-Path $dir.FullName 'sample.json'
            $samples = @()
            if (Test-Path -LiteralPath $samplePath) {
                $raw = Get-Content -LiteralPath $samplePath -Raw | ConvertFrom-Json
                if ($raw.iters.Count -ne $raw.times.Count) { throw "Mismatched samples: $samplePath" }
                $samples = @(for ($i = 0; $i -lt $raw.iters.Count; $i++) {
                    if ($raw.iters[$i] -le 0) { throw "Invalid iteration count: $samplePath" }
                    [double]$raw.times[$i] / [double]$raw.iters[$i]
                }) | Sort-Object
            }
            $medianNs = [double]$est.median.point_estimate
            $rows[$id] = [pscustomobject]@{
                id          = $id
                median_ns   = $medianNs
                ci_lo_ns    = [double]$est.median.confidence_interval.lower_bound
                ci_hi_ns    = [double]$est.median.confidence_interval.upper_bound
                mean_ns     = [double]$est.mean.point_estimate
                elements    = $elems
                throughput_kind = $throughputKind
                sample_count = $samples.Count
                q1_ns       = Get-Quantile $samples 0.25
                q3_ns       = Get-Quantile $samples 0.75
                worst_ns    = Get-Quantile $samples 1.0
            }
        }
    return $rows
}

function Get-Quantile {
    param([double[]]$Sorted, [double]$Probability)
    if ($Sorted.Count -eq 0) { return $null }
    $position = ($Sorted.Count - 1) * $Probability
    $low = [int][math]::Floor($position)
    $high = [int][math]::Ceiling($position)
    return $Sorted[$low] + ($Sorted[$high] - $Sorted[$low]) * ($position - $low)
}

function Format-Time {
    param([double]$Ns)
    if ($Ns -ge 1e9) { return ('{0:N3} s'  -f ($Ns / 1e9)) }
    if ($Ns -ge 1e6) { return ('{0:N3} ms' -f ($Ns / 1e6)) }
    if ($Ns -ge 1e3) { return ('{0:N3} us' -f ($Ns / 1e3)) }
    return ('{0:N1} ns' -f $Ns)
}

function Format-Throughput {
    param([double]$Ns, $Elements, [string]$Kind = 'elements')
    if ($null -eq $Elements -or $Ns -le 0) { return '' }
    $eps = $Elements / ($Ns / 1e9)
    if ($Kind -eq 'bytes') {
        if ($eps -ge 1GB) { return ('{0:N2} GiB/s' -f ($eps / 1GB)) }
        if ($eps -ge 1MB) { return ('{0:N2} MiB/s' -f ($eps / 1MB)) }
        if ($eps -ge 1KB) { return ('{0:N2} KiB/s' -f ($eps / 1KB)) }
        return ('{0:N2} B/s' -f $eps)
    }
    if ($eps -ge 1e9) { return ('{0:N2} Gelem/s' -f ($eps / 1e9)) }
    if ($eps -ge 1e6) { return ('{0:N2} Melem/s' -f ($eps / 1e6)) }
    return ('{0:N2} Kelem/s' -f ($eps / 1e3))
}

$base = Get-Estimates -BaselineName $Baseline
if ($base.Count -eq 0) { throw "No results found for baseline '$Baseline'." }

$cmp = @{}
if ($Compare) {
    $cmp = Get-Estimates -BaselineName $Compare
    if ($cmp.Count -eq 0) { throw "No results found for baseline '$Compare'." }
}

$table = foreach ($id in ($base.Keys | Sort-Object)) {
    if ($Filter -and $id -notmatch $Filter) { continue }
    $b = $base[$id]

    $row = [ordered]@{
        benchmark  = $id
        median     = Format-Time $b.median_ns
        ci         = '[{0} .. {1}]' -f (Format-Time $b.ci_lo_ns), (Format-Time $b.ci_hi_ns)
        throughput = Format-Throughput $b.median_ns $b.elements $b.throughput_kind
        median_ns  = $b.median_ns
        q1_ns      = $b.q1_ns
        q3_ns      = $b.q3_ns
        worst_batch_mean_ns = $b.worst_ns
        samples    = $b.sample_count
    }

    if ($Compare) {
        if ($cmp.ContainsKey($id)) {
            $c = $cmp[$id]
            $delta = (($c.median_ns - $b.median_ns) / $b.median_ns) * 100.0
            $row['after']      = Format-Time $c.median_ns
            $row['after_tput'] = Format-Throughput $c.median_ns $c.elements $c.throughput_kind
            $row['delta_pct']  = '{0:+0.0;-0.0;0.0}' -f $delta
            $row['speedup']    = '{0:N2}x' -f ($b.median_ns / [math]::Max($c.median_ns, 1e-9))
            $row['gate']       = if ($delta -gt 10) { 'REGRESSION >10%' }
                                 elseif ($delta -lt -10) { 'improved >10%' }
                                 else { '' }
        } else {
            $row['after']      = '(absent)'
            $row['after_tput'] = ''
            $row['delta_pct']  = ''
            $row['speedup']    = ''
            $row['gate']       = 'new/removed'
        }
    }

    [pscustomobject]$row
}

$table | Format-Table -AutoSize

if ($Compare) {
    $regressions = @($table | Where-Object { $_.gate -eq 'REGRESSION >10%' })
    Write-Host ''
    if ($regressions.Count -gt 0) {
        Write-Host "$($regressions.Count) cell(s) past the 10% regression gate (CLAUDE.md Sec. 3) - profile before accepting." -ForegroundColor Red
    } else {
        Write-Host 'No cell regressed by more than 10%.' -ForegroundColor Green
    }
}

if ($Csv) {
    $table | Export-Csv -Path $Csv -NoTypeInformation -Encoding utf8
    Write-Host "Wrote $Csv" -ForegroundColor DarkGray
}

if ($PassThru) { $table }
