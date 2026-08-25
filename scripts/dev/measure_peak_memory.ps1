<#
.SYNOPSIS
    Measure peak working set and peak paged memory of a benchmark process.

.DESCRIPTION
    CLAUDE.md Sec. 3 requires peak memory, not instantaneous, and Sec. 4 sets a 16 GB
    cap. On Linux that cap is enforced with `systemd-run -p MemoryMax=16G`
    against RSS; Windows has no equivalent RSS cgroup gate, so here the cap is
    *observed and asserted* rather than kernel-enforced. This script is the
    observation half.

    It launches the target with `Start-Process -PassThru`, waits for exit, and
    then reads `PeakWorkingSet64` / `PeakPagedMemorySize64` from the exited
    process object. .NET caches both counters at exit, so this is a true peak
    over the whole run - not a sampled instantaneous value that could miss the
    high-water mark between polls.

    Note on comparability: the pre-existing ~69 MiB figure on this host was
    measured on `interning_bench`. The `csr_bench` / `hnn_bench` targets
    allocate deliberately larger synthetic workloads, so their absolute peak is
    higher by construction. Compare a target against ITSELF across a code
    change; do not compare across targets.

.PARAMETER Bench
    Bench target name (e.g. csr_bench, hnn_bench, interning_bench).

.PARAMETER Filter
    Optional Criterion filter regex, to restrict which cells run.

.PARAMETER Threads
    Optional RAYON_NUM_THREADS value for the child process.

.PARAMETER CargoProfile
    Cargo profile passed through as --profile (default: bench).

.EXAMPLE
    .\measure_peak_memory.ps1 -Bench hnn_bench -Filter 'conv_hgnn'

.EXAMPLE
    .\measure_peak_memory.ps1 -Bench csr_bench -Threads 24
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Bench,
    [string]$Filter = '',
    [int]$Threads = 0,
    [string]$CargoProfile = 'bench'
)

$ErrorActionPreference = 'Stop'
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
Push-Location $repoRoot
try {
    # Build first so compilation cost is not attributed to the measured run.
    #
    # Windows PowerShell 5.1 turns a native command's stderr into ErrorRecords,
    # and cargo writes its progress there -- under $ErrorActionPreference='Stop'
    # that aborts the script on a perfectly successful build. Drop to
    # 'Continue' for the duration of the call and gate on $LASTEXITCODE, which
    # is the reliable signal.
    Write-Host "Building bench target '$Bench' ..." -ForegroundColor Cyan
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    & cargo build --profile $CargoProfile --bench $Bench --workspace *>&1 | Out-Null
    $buildCode = $LASTEXITCODE
    $ErrorActionPreference = $prevEap
    if ($buildCode -ne 0) { throw "cargo build failed with exit code $buildCode" }

    # Locate the built executable. Cargo mangles bench binaries with a hash
    # suffix; take the newest match.
    $benchDir = Join-Path $repoRoot "target\release\deps"
    $exe = Get-ChildItem -Path $benchDir -Filter "$Bench-*.exe" -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
    if (-not $exe) { throw "Could not locate a built binary for bench '$Bench' under $benchDir" }

    $benchArgs = @('--bench')
    if ($Filter) { $benchArgs += $Filter }

    if ($Threads -gt 0) {
        $env:RAYON_NUM_THREADS = "$Threads"
        Write-Host "RAYON_NUM_THREADS=$Threads" -ForegroundColor DarkGray
    }

    Write-Host "Running $($exe.Name) $($benchArgs -join ' ') ..." -ForegroundColor Cyan
    $sw = [System.Diagnostics.Stopwatch]::StartNew()

    # NOTE: deliberately NOT Start-Process. The Process object that cmdlet
    # hands back reports PeakWorkingSet64 = 0 whether or not -Wait is used - it
    # is not the same object .NET populates from the OS. Starting the process
    # through System.Diagnostics.Process directly keeps the handle we own, and
    # the peak counters are then readable after exit (Windows keeps them alive
    # for the process object until the handle is released).
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $exe.FullName
    # Quote the filter: Criterion regexes routinely contain '|'.
    $psi.Arguments = ($benchArgs | ForEach-Object { '"' + $_ + '"' }) -join ' '
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $false
    $psi.RedirectStandardError = $false

    $proc = [System.Diagnostics.Process]::Start($psi)

    # The peak counters are maintained by the OS for the lifetime of the
    # process and become unreadable (0) once it exits, so they must be sampled
    # while it is alive. This is NOT a sampled approximation of the peak: each
    # read returns the OS-tracked high-water mark since process start, so
    # reading periodically and keeping the maximum yields the true peak
    # regardless of poll interval. Only a spike inside the final poll window
    # could be missed, which the post-exit read below also tries to catch.
    $peakWs = 0L; $peakPaged = 0L; $peakVm = 0L
    while (-not $proc.HasExited) {
        try {
            $proc.Refresh()
            if ($proc.PeakWorkingSet64      -gt $peakWs)    { $peakWs    = $proc.PeakWorkingSet64 }
            if ($proc.PeakPagedMemorySize64 -gt $peakPaged) { $peakPaged = $proc.PeakPagedMemorySize64 }
            if ($proc.PeakVirtualMemorySize64 -gt $peakVm)  { $peakVm    = $proc.PeakVirtualMemorySize64 }
        } catch {
            # Process exited between HasExited and Refresh; the last sample stands.
        }
        Start-Sleep -Milliseconds 100
    }
    $proc.WaitForExit()
    $sw.Stop()

    $peakWsMiB    = [math]::Round($peakWs    / 1MB, 2)
    $peakPagedMiB = [math]::Round($peakPaged / 1MB, 2)
    $peakVmMiB    = [math]::Round($peakVm    / 1MB, 2)

    $result = [pscustomobject]@{
        bench             = $Bench
        filter            = $(if ($Filter) { $Filter } else { '(all)' })
        rayon_threads     = $(if ($Threads -gt 0) { $Threads } else { 'default' })
        exit_code         = $proc.ExitCode
        wall_seconds      = [math]::Round($sw.Elapsed.TotalSeconds, 2)
        peak_working_set_mib = $peakWsMiB
        peak_paged_mib       = $peakPagedMiB
        peak_virtual_mib     = $peakVmMiB
        cap_16gb_ok          = ($peakWsMiB -lt 16384)
    }

    $result | Format-List
    if (-not $result.cap_16gb_ok) {
        throw "Peak working set $peakWsMiB MiB exceeds the CLAUDE.md Sec. 4 16 GB cap - abort and redesign, do not raise the cap."
    }
    $result
}
finally {
    Remove-Item Env:\RAYON_NUM_THREADS -ErrorAction SilentlyContinue
    Pop-Location
}
