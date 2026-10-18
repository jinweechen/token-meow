param(
    [string]$InstallerPath,
    [string]$ApplicationPath,
    [string]$GeneratedTemplatePath,
    [string]$OutputPath
)

# Read-only artifact and registration inspection. This script never installs,
# uninstalls, launches, stops a process, modifies the registry, or removes data.
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
$configuration = Get-Content -LiteralPath (Join-Path $taskRoot 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json

function Read-PeMachine([string]$LiteralPath) {
    $stream = [IO.File]::Open($LiteralPath, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)
    $reader = [IO.BinaryReader]::new($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5A4D) { throw 'Not a PE executable' }
        $stream.Position = 0x3C
        $peOffset = $reader.ReadUInt32()
        if ($peOffset -gt $stream.Length - 6) { throw 'Invalid PE header offset' }
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) { throw 'Invalid PE signature' }
        $machine = $reader.ReadUInt16()
        switch ($machine) {
            0x8664 { return 'x64' }
            0x014C { return 'x86' }
            0xAA64 { return 'arm64' }
            default { return ('0x{0:X4}' -f $machine) }
        }
    } finally { $reader.Dispose() }
}

function Read-Artifact([string]$LiteralPath) {
    if (-not $LiteralPath) { return $null }
    $file = Get-Item -LiteralPath $LiteralPath
    if ($file.PSIsContainer) { throw 'Expected an executable file' }
    $signature = Get-AuthenticodeSignature -LiteralPath $file.FullName
    return [ordered]@{
        path = $file.FullName
        bytes = $file.Length
        sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
        peMachine = Read-PeMachine $file.FullName
        signatureStatus = [string]$signature.Status
        productVersion = $file.VersionInfo.ProductVersion
    }
}

$checks = [Collections.Generic.List[object]]::new()
function Add-Check([string]$Name, [bool]$Passed, [string]$Basis) {
    $checks.Add([ordered]@{ name = $Name; status = $(if ($Passed) { 'passed' } else { 'failed' }); basis = $Basis })
}
Add-Check 'current-user-only' ($configuration.bundle.windows.nsis.installMode -eq 'currentUser') 'tauri.conf.json'
Add-Check 'in-place-downgrade-disabled' ($configuration.bundle.windows.allowDowngrades -eq $false) 'tauri.conf.json; explicit uninstall followed by older installation is still possible'
Add-Check 'webview-download-bootstrapper' ($configuration.bundle.windows.webviewInstallMode.type -eq 'downloadBootstrapper') 'tauri.conf.json; missing-runtime installs require network'
Add-Check 'nsis-only' (@($configuration.bundle.targets).Count -eq 1 -and $configuration.bundle.targets[0] -eq 'nsis') 'tauri.conf.json'
$hooksFile = Join-Path (Join-Path $taskRoot 'src-tauri') $configuration.bundle.windows.nsis.installerHooks
$hooksText = Get-Content -LiteralPath $hooksFile -Raw
Add-Check 'scoped-run-upgrade-hooks' ($configuration.productName -eq 'TokenMeow' -and $hooksText.Contains('MUI_CUSTOMFUNCTION_GUIINIT TM_CaptureUpgradeRun') -and $hooksText.Contains('${If} $TM_PreviousRun S== $2') -and $hooksText.Contains('${AndIf} $TM_RunReadValue S== $TM_PreviousRun') -and $hooksText.Contains('${If} $TM_RunReadStatus = 2')) 'pinned stock template plus custom hook source; runtime upgrade remains untested'
$configuredTemplate = Get-Content -LiteralPath (Join-Path (Join-Path $taskRoot 'src-tauri') $configuration.bundle.windows.nsis.template) -Raw
Add-Check 'ordinary-uninstall-removes-only-own-run' ($configuredTemplate.Contains('${If} $TM_RunReadStatus = 0') -and $configuredTemplate.Contains('${AndIf} $TM_RunReadValue S== ''$\"$INSTDIR\${MAINBINARYNAME}.exe$\" --background''') -and $hooksText.Contains('i 0x30000002')) 'pinned template condition plus REG_SZ-only read; user-replaced values and registry types are left untouched'

$installer = Read-Artifact $InstallerPath
$application = Read-Artifact $ApplicationPath
if ($application) { Add-Check 'payload-windows-x64' ($application.peMachine -eq 'x64') 'application PE header; the NSIS wrapper may legitimately be x86' }
$template = $null
if ($GeneratedTemplatePath) {
    $file = Get-Item -LiteralPath $GeneratedTemplatePath
    $templateText = Get-Content -LiteralPath $file.FullName -Raw
    $template = [ordered]@{ path = $file.FullName; sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash }
    Add-Check 'generated-product-identity' ($templateText.Contains('!define PRODUCTNAME "' + $configuration.productName + '"') -and $templateText.Contains('!define BUNDLEID "' + $configuration.identifier + '"')) 'actual generated template; rejects other projects in a shared Cargo target'
    Add-Check 'generated-x64-payload' ($templateText.Contains('!define ARCH "x64"')) 'actual generated NSIS architecture define'
    Add-Check 'generated-in-place-downgrade-disabled' ($templateText.Contains('!define ALLOWDOWNGRADES "false"')) 'actual generated NSIS policy define; explicit uninstall is separate'
    Add-Check 'data-removal-is-selectable' ($templateText.Contains('Var DeleteAppDataCheckbox') -and $templateText.Contains('$DeleteAppDataCheckboxState = 1')) 'static template branch only; checkbox behavior remains a runtime test'
    Add-Check 'update-preserves-data-and-run-branches' ($templateText.Contains('${If} $UpdateMode <> 1') -and $templateText.Contains('${AndIf} $UpdateMode <> 1') -and $templateText.Contains('DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"')) 'static branch presence; not an upgrade execution receipt'
    Add-Check 'data-removal-scoped-to-bundle-id' ($templateText.Contains('RmDir /r "$LOCALAPPDATA\${BUNDLEID}"') -and $templateText.Contains('RmDir /r "$APPDATA\${BUNDLEID}"')) 'no custom removal of Codex data or environment-overridden data directories'
    Add-Check 'missing-webview-failure-aborts' ($templateText.Contains('NSISdl::download') -and $templateText.Contains('Abort "$(webview2AbortError)"')) 'static Microsoft bootstrapper failure path'
}

$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\' + $configuration.productName
$registration = if (Test-Path -LiteralPath $uninstallKey) { Get-ItemProperty -LiteralPath $uninstallKey } else { $null }
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$runValue = if (Test-Path -LiteralPath $runKey) { (Get-ItemProperty -LiteralPath $runKey).PSObject.Properties[$configuration.productName] } else { $null }
$report = [ordered]@{
    capturedAt = [DateTimeOffset]::UtcNow.ToString('O')
    qualification = 'read-only-static-inspection'
    product = $configuration.productName
    version = $configuration.version
    identifier = $configuration.identifier
    installer = $installer
    application = $application
    generatedTemplate = $template
    checks = @($checks.ToArray())
    currentUserRegistration = [ordered]@{ present = [bool]$registration; displayVersion = $(if ($registration) { $registration.DisplayVersion } else { $null }); runValuePresent = [bool]$runValue }
    runtimeNotChecked = @('clean installation', 'upgrade and committed ledger preservation', 'uninstall with data kept', 'uninstall with explicit data deletion', 'missing WebView2 online/offline', 'installed AUMID toast', 'startup at next Windows login')
}
$json = $report | ConvertTo-Json -Depth 8
if ($OutputPath) {
    $destination = [IO.Path]::GetFullPath($OutputPath)
    $parentDirectory = Split-Path -Parent $destination
    if (-not (Test-Path -LiteralPath $parentDirectory)) { New-Item -ItemType Directory -Path $parentDirectory -Force | Out-Null }
    [IO.File]::WriteAllText($destination, $json, [Text.UTF8Encoding]::new($false))
}
$json
if (@($checks | Where-Object status -eq 'failed').Count -gt 0) { exit 2 }
