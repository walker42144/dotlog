# Windows formal release: remap local build paths and verify the packaged program.
$ErrorActionPreference = "Stop"
$releaseRoot = Split-Path -Parent $PSScriptRoot
$profileRoot = [Environment]::GetFolderPath('UserProfile')
$priorEncoded = [Environment]::GetEnvironmentVariable('CARGO_ENCODED_RUSTFLAGS', 'Process')
$priorPreview = [Environment]::GetEnvironmentVariable('DOTLOG_PREVIEW', 'Process')
$priorProfile = [Environment]::GetEnvironmentVariable('DOTLOG_PROFILE_DIR', 'Process')
$priorConfig = [Environment]::GetEnvironmentVariable('TAURI_CONFIG', 'Process')
$flags = @(
    "--remap-path-prefix=$profileRoot=/build/user",
    "--remap-path-prefix=$($profileRoot.Replace('\', '/'))=/build/user",
    "--remap-path-prefix=$releaseRoot=/src/dotlog",
    "--remap-path-prefix=$($releaseRoot.Replace('\', '/'))=/src/dotlog"
)
Push-Location -LiteralPath $releaseRoot
try {
    $encoded = $flags -join [char]31
    if ($priorEncoded) { $encoded = $priorEncoded + [char]31 + $encoded }
    $env:CARGO_ENCODED_RUSTFLAGS = $encoded
    Remove-Item Env:DOTLOG_PREVIEW, Env:DOTLOG_PROFILE_DIR, Env:TAURI_CONFIG -ErrorAction SilentlyContinue
    & npm.cmd run tauri --workspace @dotlog/desktop -- build --bundles nsis
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    $targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $releaseRoot 'target' }
    $exe = Join-Path $targetRoot 'release/dotlog-desktop.exe'
    $bytes = [IO.File]::ReadAllBytes($exe)
    $texts = @([Text.Encoding]::UTF8.GetString($bytes), [Text.Encoding]::Unicode.GetString($bytes))
    foreach ($text in $texts) {
        foreach ($prefix in @($profileRoot, $profileRoot.Replace('\', '/'))) {
            if ($text.IndexOf($prefix, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
                throw 'Local user build path remains in the executable. Do not publish this build.'
            }
        }
    }
    Write-Host 'Release build passed the local user-path check.'
} finally {
    [Environment]::SetEnvironmentVariable('CARGO_ENCODED_RUSTFLAGS', $priorEncoded, 'Process')
    [Environment]::SetEnvironmentVariable('DOTLOG_PREVIEW', $priorPreview, 'Process')
    [Environment]::SetEnvironmentVariable('DOTLOG_PROFILE_DIR', $priorProfile, 'Process')
    [Environment]::SetEnvironmentVariable('TAURI_CONFIG', $priorConfig, 'Process')
    Pop-Location
}
