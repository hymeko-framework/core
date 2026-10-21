<#
.SYNOPSIS
    Measure a Criterion target or an existing executable under a working-set cap.
.DESCRIPTION
    Polls OS high-water counters every 100 ms and terminates the child if the
    observed peak exceeds the cap (at most 16 GiB). This is a userspace guard,
    not a kernel RSS quota. A peak inside the final poll window may be missed;
    zero means no reliable observation, never a measured zero-memory process.
    Compilation is outside the measured span. VAS is recorded, never capped.
    LogPrefix saves stdout, stderr and metrics; otherwise output is inherited.
    Rayon configuration applies only to the child.
.PARAMETER ProcessorAffinity
    Optional process-local logical-processor bitmask, applied before warm-up.
    Zero keeps OS scheduling. The requested nonzero mask is read back and checked.
.EXAMPLE
    .\measure_peak_memory.ps1 -Bench csr_bench -Filter csr_spmv -Threads 24
.EXAMPLE
    .\measure_peak_memory.ps1 -Executable .\probe.exe -ArgumentString '--verify' -LogPrefix .\probe
#>
[CmdletBinding(DefaultParameterSetName = 'Bench')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'Bench')][string]$Bench,
    [Parameter(ParameterSetName = 'Bench')][string]$Filter = '',
    [Parameter(ParameterSetName = 'Bench')][string]$CargoProfile = 'bench',
    [Parameter(Mandatory = $true, ParameterSetName = 'Program')][string]$Executable,
    [Parameter(ParameterSetName = 'Program')][string]$ArgumentString = '',
    [string]$WorkingDirectory = (Get-Location).Path,
    [int]$Threads = 0,
    [ValidateRange(0, [long]::MaxValue)][long]$ProcessorAffinity = 0,
    [string]$LogPrefix = '',
    [ValidateRange(1, 16384)][int]$MaxWorkingSetMiB = 16384
)
$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$runDirectory = (Resolve-Path -LiteralPath $WorkingDirectory).Path
if ($LogPrefix) { $LogPrefix = [IO.Path]::GetFullPath($LogPrefix) }
if ($PSCmdlet.ParameterSetName -eq 'Bench') {
    Push-Location $repoRoot
    try {
        Write-Host "Building bench target '$Bench' ..."
        $previousPreference = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        & cargo build --locked --offline --profile $CargoProfile --bench $Bench --workspace *>&1 | Out-Null
        $buildCode = $LASTEXITCODE
        $ErrorActionPreference = $previousPreference
        if ($buildCode -ne 0) { throw "cargo build failed: $buildCode" }
        $profileDirectory = if ($CargoProfile -eq 'dev') { 'debug' } elseif ($CargoProfile -eq 'bench') { 'release' } else { $CargoProfile }
        $benchDirectory = Join-Path $repoRoot "target\$profileDirectory\deps"
        $exe = Get-ChildItem -LiteralPath $benchDirectory -Filter "$Bench-*.exe" |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if (-not $exe) { throw "No executable for '$Bench'" }
        $ArgumentString = '--bench'
        if ($Filter) { $ArgumentString += ' "' + ($Filter -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1') + '"' }
        $runDirectory = $repoRoot
    } finally { Pop-Location }
} else {
    $exe = Get-Item -LiteralPath $Executable
}
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $exe.FullName
$psi.Arguments = $ArgumentString
$psi.WorkingDirectory = $runDirectory
$psi.UseShellExecute = $false
$psi.CreateNoWindow = $true
$psi.RedirectStandardOutput = [bool]$LogPrefix
$psi.RedirectStandardError = [bool]$LogPrefix
if ($Threads -gt 0) { $psi.EnvironmentVariables['RAYON_NUM_THREADS'] = "$Threads" }
Write-Host "Running $($exe.Name) $ArgumentString"
$watch = [Diagnostics.Stopwatch]::StartNew()
$proc = [Diagnostics.Process]::Start($psi)
$peakWs = 0L; $peakPaged = 0L; $peakVm = 0L
$terminatedForCap = $false
$nextProgress = 10.0
try {
    # Optional process-local scheduling control; zero preserves the OS default.
    # Set it before fixture preparation/warm-up, and retain cleanup if it fails.
    if ($ProcessorAffinity -gt 0) {
        $proc.ProcessorAffinity = [IntPtr]$ProcessorAffinity
        if ($proc.ProcessorAffinity.ToInt64() -ne $ProcessorAffinity) {
            throw "Requested processor affinity was not applied."
        }
    }
    if ($LogPrefix) {
        # Drain both pipes concurrently; large diagnostics cannot deadlock the child.
        $stdout = $proc.StandardOutput.ReadToEndAsync()
        $stderr = $proc.StandardError.ReadToEndAsync()
    }
    while (-not $proc.HasExited) {
        try {
            $proc.Refresh()
            $peakWs = [math]::Max($peakWs, $proc.PeakWorkingSet64)
            $peakPaged = [math]::Max($peakPaged, $proc.PeakPagedMemorySize64)
            $peakVm = [math]::Max($peakVm, $proc.PeakVirtualMemorySize64)
        } catch { } # Exit races leave the last observation.
        if ($peakWs -gt ($MaxWorkingSetMiB * 1MB) -and -not $proc.HasExited) {
            $terminatedForCap = $true
            $proc.Kill()
        }
        if ($watch.Elapsed.TotalSeconds -ge $nextProgress) {
            Write-Host ("PROGRESS {0}: {1:N1}s, observed peak {2:N1} MiB" -f $exe.Name, $watch.Elapsed.TotalSeconds, ($peakWs / 1MB))
            $nextProgress += 10.0
        }
        Start-Sleep -Milliseconds 100
    }
    $proc.WaitForExit()
    $watch.Stop()
    $result = [pscustomobject]@{
        bench = $Bench
        executable = $exe.FullName
        executable_sha256 = (Get-FileHash -LiteralPath $exe.FullName -Algorithm SHA256).Hash
        arguments = $ArgumentString
        working_directory = $runDirectory
        filter = $(if ($Filter) { $Filter } else { '(all)' })
        rayon_threads = $(if ($Threads -gt 0) { $Threads } else { 'inherited' })
        processor_affinity_mask = $(if ($ProcessorAffinity -gt 0) { $ProcessorAffinity } else { 'OS-managed' })
        exit_code = $proc.ExitCode
        wall_seconds = [math]::Round($watch.Elapsed.TotalSeconds, 3)
        peak_working_set_mib = [math]::Round($peakWs / 1MB, 3)
        peak_paged_mib = [math]::Round($peakPaged / 1MB, 3)
        peak_virtual_mib = [math]::Round($peakVm / 1MB, 3)
        peak_observed = ($peakWs -gt 0)
        poll_interval_ms = 100
        cap_mib = $MaxWorkingSetMiB
        terminated_for_cap = $terminatedForCap
        cap_ok = (-not $terminatedForCap -and $peakWs -le ($MaxWorkingSetMiB * 1MB))
        cap_16gb_ok = ($peakWs -le 16GB)
    }
    if ($LogPrefix) {
        $utf8 = New-Object Text.UTF8Encoding $false
        [IO.File]::WriteAllText("$LogPrefix.stdout.log", $stdout.Result, $utf8)
        [IO.File]::WriteAllText("$LogPrefix.stderr.log", $stderr.Result, $utf8)
        [IO.File]::WriteAllText("$LogPrefix.memory.json", ($result | ConvertTo-Json), $utf8)
    }
    $result
    if (-not $result.cap_ok) { throw "Working-set cap breached: $($result.peak_working_set_mib) MiB; child stopped." }
} finally {
    if (-not $proc.HasExited) { $proc.Kill(); $proc.WaitForExit() }
    $proc.Dispose()
}
