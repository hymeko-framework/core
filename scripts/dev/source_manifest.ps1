<#
.SYNOPSIS
    Deterministic SHA-256 manifest of everything that affects a measurement.

.DESCRIPTION
    The working tree is uncommitted, so a git SHA does not identify what was
    measured. This produces a content hash over every source, shader, manifest
    and lockfile that can change a benchmark result, plus a single rolled-up
    manifest hash.

    Two uses:
      1. Local provenance - a benchmark report cites the manifest hash, so a
         later reader can tell whether the tree has moved.
      2. Cross-host comparison - a remote benchmark result is only comparable
         to a local one if both cite the SAME manifest hash. Without that,
         "the other machine was faster" may just mean it ran different code.

    Deliberately EXCLUDED: target/, Criterion output, scratchpad, .git, and the
    vendor tree. None of them change the semantics of what is measured, and all
    of them are large.

.PARAMETER Json
    Also write the per-file table as JSON to this path.

.EXAMPLE
    .\source_manifest.ps1
    .\source_manifest.ps1 -Json manifest.json
#>
[CmdletBinding()]
param(
    [string]$Json = ''
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

# Directories walked recursively, with the extensions that matter.
# NOTE: PowerShell does not treat `**` as a recursive glob, so this walks
# directories with -Recurse and filters by extension instead. An earlier version
# used `src\**\*.rs` and silently matched only 30 files.
$roots = @(
    'hymeko_core\src', 'hymeko_core\benches', 'hymeko_core\tests',
    'hymeko_hnn\src', 'hymeko_hnn\benches', 'hymeko_hnn\examples',
    'hymeko_wgpu\src', 'hymeko_wgpu\benches', 'hymeko_wgpu\tests', 'hymeko_wgpu\shaders'
)
$extensions = @('.rs', '.wgsl')

$files = foreach ($r in $roots) {
    $full = Join-Path $repoRoot $r
    if (Test-Path $full) {
        Get-ChildItem -Path $full -File -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $extensions -contains $_.Extension }
    }
}

# Manifests and the lockfile pin dependency versions and feature sets, both of
# which change what is measured.
$files += @('Cargo.toml', 'Cargo.lock',
            'hymeko_core\Cargo.toml', 'hymeko_hnn\Cargo.toml', 'hymeko_wgpu\Cargo.toml') |
    ForEach-Object {
        $p = Join-Path $repoRoot $_
        if (Test-Path $p) { Get-Item $p }
    }

$files = $files |
    Where-Object { $_.FullName -notmatch '\\target\\|\\\.git\\|\\scratchpad\\' } |
    Sort-Object FullName -Unique

$rows = foreach ($f in $files) {
    # Normalise the path so Windows and Linux produce identical manifest input.
    $rel = $f.FullName.Substring($repoRoot.Length).TrimStart('\', '/') -replace '\\', '/'
    [pscustomobject]@{
        path   = $rel
        bytes  = $f.Length
        sha256 = (Get-FileHash $f.FullName -Algorithm SHA256).Hash.ToLower()
    }
}
$rows = $rows | Sort-Object path

# Roll-up: hash of "sha256  path" lines, LF-joined, so the manifest hash is
# itself reproducible across platforms.
$joined = ($rows | ForEach-Object { "$($_.sha256)  $($_.path)" }) -join "`n"
$sha = [System.Security.Cryptography.SHA256]::Create()
$manifestHash = ([BitConverter]::ToString(
    $sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($joined))
) -replace '-', '').ToLower()

"files:         $($rows.Count)"
"total bytes:   $(($rows | Measure-Object bytes -Sum).Sum)"
"MANIFEST HASH: $manifestHash"
""
"short:         $($manifestHash.Substring(0,12))"

if ($Json) {
    [pscustomobject]@{
        manifest_hash = $manifestHash
        file_count    = $rows.Count
        generated_at  = (Get-Date -Format 'yyyy-MM-ddTHH:mm:ssK')
        files         = $rows
    } | ConvertTo-Json -Depth 4 | Set-Content $Json -Encoding utf8
    "wrote $Json"
}
