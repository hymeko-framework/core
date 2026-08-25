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
    [switch]$PassThru
)

$ErrorActionPreference = 'Stop'
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$critRoot = Join-Path $repoRoot 'target\criterion'
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
            if (Test-Path $benchJson) {
                $meta = Get-Content $benchJson -Raw | ConvertFrom-Json
                $id = $meta.full_id
                $elems = $null
                if ($meta.throughput) {
                    if ($meta.throughput.Elements)   { $elems = [double]$meta.throughput.Elements }
                    elseif ($meta.throughput.Bytes)  { $elems = [double]$meta.throughput.Bytes }
                }
            } else {
                # Fall back to the directory path relative to target/criterion.
                $id = $dir.Parent.FullName.Substring($critRoot.Length).TrimStart('\') -replace '\\', '/'
                $elems = $null
            }

            $medianNs = [double]$est.median.point_estimate
            $rows[$id] = [pscustomobject]@{
                id          = $id
                median_ns   = $medianNs
                ci_lo_ns    = [double]$est.median.confidence_interval.lower_bound
                ci_hi_ns    = [double]$est.median.confidence_interval.upper_bound
                mean_ns     = [double]$est.mean.point_estimate
                elements    = $elems
            }
        }
    return $rows
}

function Format-Time {
    param([double]$Ns)
    if ($Ns -ge 1e9) { return ('{0:N3} s'  -f ($Ns / 1e9)) }
    if ($Ns -ge 1e6) { return ('{0:N3} ms' -f ($Ns / 1e6)) }
    if ($Ns -ge 1e3) { return ('{0:N3} us' -f ($Ns / 1e3)) }
    return ('{0:N1} ns' -f $Ns)
}

function Format-Throughput {
    param([double]$Ns, $Elements)
    if ($null -eq $Elements -or $Ns -le 0) { return '' }
    $eps = $Elements / ($Ns / 1e9)
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
        throughput = Format-Throughput $b.median_ns $b.elements
    }

    if ($Compare) {
        if ($cmp.ContainsKey($id)) {
            $c = $cmp[$id]
            $delta = (($c.median_ns - $b.median_ns) / $b.median_ns) * 100.0
            $row['after']      = Format-Time $c.median_ns
            $row['after_tput'] = Format-Throughput $c.median_ns $c.elements
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
