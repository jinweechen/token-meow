# Rebuild only this application's four bundled icon files from its original SVG.
# Requires the already locked local @tauri-apps/cli; does not download artwork.
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
$source = Join-Path $taskRoot 'src-tauri\icons\source.svg'
$generated = Join-Path $taskRoot 'output\icon-generation'
Push-Location $taskRoot
try {
    & pnpm exec tauri icon $source --output $generated
    if ($LASTEXITCODE -ne 0) { throw 'Tauri icon generation failed' }
    foreach ($name in @('32x32.png', '128x128.png', 'icon.ico')) {
        Copy-Item -LiteralPath (Join-Path $generated $name) -Destination (Join-Path $taskRoot ('src-tauri\icons\' + $name)) -Force
    }
    Copy-Item -LiteralPath (Join-Path $generated '128x128@2x.png') -Destination (Join-Path $taskRoot 'src-tauri\icons\icon.png') -Force
} finally { Pop-Location }
