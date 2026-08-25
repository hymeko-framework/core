<#
.SYNOPSIS
    Rayon thread-scaling sweep over a Criterion bench target.

.DESCRIPTION
    Rayon's global thread pool is process-wide and initialised once, so the
    thread count cannot be varied between benchmark groups inside a single
    process. Each thread count therefore gets its OWN process invocation, with
    RAYON_NUM_THREADS set in the environment before launch. Sweeping inside one
    process would silently measure the first pool size for every cell - a
    failure mode this script exists to prevent.

    Each run saves a distinct Criterion baseline named `t<N>`, so results stay
    on disk and can be compared afterwards without re-running:

        cargo bench --bench <b> -- --baseline t1

.PARAMETER Bench
    Bench target name (csr_bench | hnn_bench).

.PARAMETER Filter
    Criterion filter regex restricting which cells run. Strongly recommended -
    a full sweep of every cell at five thread counts is long. Default targets
    the parallel-capable kernels only.

.PARAMETER Threads
    Thread counts to sweep. Default 1, 4, 8, 16, 24 (this host has 24 logical
    cores).

.EXAMPLE
    .\bench_thread_sweep.ps1 -Bench hnn_bench -Filter 'mp_(gather|scatter)'

.EXAMPLE
    .\bench_thread_sweep.ps1 -Bench csr_bench -Threads 1,8,24
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateSet('csr_bench', 'hnn_bench')][string]$Bench,
    [string]$Filter = '',
    [int[]]$Threads = @(1, 4, 8, 16, 24)
)

$ErrorActionPreference = 'Stop'
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
Push-Location $repoRoot
try {
    Write-Host "Thread sweep: bench=$Bench filter='$Filter' threads=$($Threads -join ',')" -ForegroundColor Cyan
    Write-Host "Host must be quiet for the duration (CLAUDE.md Sec. 3)." -ForegroundColor DarkGray

    $summary = @()
    foreach ($t in $Threads) {
        $env:RAYON_NUM_THREADS = "$t"
        $baseline = "t$t"
        Write-Host "`n=== RAYON_NUM_THREADS=$t  (baseline '$baseline') ===" -ForegroundColor Yellow

        $benchArgs = @('bench', '--bench', $Bench, '--', '--save-baseline', $baseline)
        if ($Filter) { $benchArgs += @('--', $Filter) }

        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        if ($Filter) {
            & cargo bench --bench $Bench -- $Filter --save-baseline $baseline
        } else {
            & cargo bench --bench $Bench -- --save-baseline $baseline
        }
        $sw.Stop()
        if ($LASTEXITCODE -ne 0) { throw "cargo bench failed at threads=$t (exit $LASTEXITCODE)" }

        $summary += [pscustomobject]@{
            threads      = $t
            baseline     = $baseline
            wall_seconds = [math]::Round($sw.Elapsed.TotalSeconds, 1)
        }
    }

    Write-Host "`n=== Sweep complete ===" -ForegroundColor Green
    $summary | Format-Table -AutoSize
    Write-Host "Criterion baselines are under target/criterion/**/t<N>/." -ForegroundColor DarkGray
    Write-Host "Compare with:  cargo bench --bench $Bench -- --baseline t1" -ForegroundColor DarkGray
    $summary
}
finally {
    Remove-Item Env:\RAYON_NUM_THREADS -ErrorAction SilentlyContinue
    Pop-Location
}
