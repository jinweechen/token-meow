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
function Remove-NsisComments([string]$Text) {
    # The scoped engine sources use whole-line comments. Ignore those when
    # looking for active shutdown calls; unused stock utils macros are not calls.
    return [regex]::Replace($Text, '(?m)^\s*[;#][^\r\n]*', '')
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
$generatedSupportingSources = @()
if ($GeneratedTemplatePath) {
    $file = Get-Item -LiteralPath $GeneratedTemplatePath
    $templateText = Get-Content -LiteralPath $file.FullName -Raw
    $template = [ordered]@{ path = $file.FullName; sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash }
    $generatedHooksText = ''
    $engineText = ''
    $hookIncludes = [regex]::Matches($templateText, '(?m)^\s*!include\s+"([^"\r\n]*installer-hooks\.nsh)"\s*$')
    if ($hookIncludes.Count -eq 1) {
        $generatedHooksPath = $hookIncludes[0].Groups[1].Value
        if (-not [IO.Path]::IsPathRooted($generatedHooksPath)) { $generatedHooksPath = Join-Path $file.DirectoryName $generatedHooksPath }
        $generatedHooksPath = [IO.Path]::GetFullPath($generatedHooksPath)
        $generatedHooksText = Get-Content -LiteralPath $generatedHooksPath -Raw
        $generatedSupportingSources += [ordered]@{ path = $generatedHooksPath; sha256 = (Get-FileHash -LiteralPath $generatedHooksPath -Algorithm SHA256).Hash }
        if ($generatedHooksText.Contains('!include "${__FILEDIR__}\installer-engine.nsh"')) {
            $enginePath = Join-Path (Split-Path -Parent $generatedHooksPath) 'installer-engine.nsh'
            $engineText = Get-Content -LiteralPath $enginePath -Raw
            $generatedSupportingSources += [ordered]@{ path = $enginePath; sha256 = (Get-FileHash -LiteralPath $enginePath -Algorithm SHA256).Hash }
        }
    }
    $activeTemplate = Remove-NsisComments $templateText
    $activeEngine = Remove-NsisComments $engineText
    $activeSources = $activeTemplate + "`n" + (Remove-NsisComments $generatedHooksText) + "`n" + $activeEngine
    Add-Check 'generated-product-identity' ($templateText.Contains('!define PRODUCTNAME "' + $configuration.productName + '"') -and $templateText.Contains('!define BUNDLEID "' + $configuration.identifier + '"')) 'actual generated template; rejects other projects in a shared Cargo target'
    Add-Check 'generated-x64-payload' ($templateText.Contains('!define ARCH "x64"')) 'actual generated NSIS architecture define'
    Add-Check 'generated-in-place-downgrade-disabled' ($templateText.Contains('!define ALLOWDOWNGRADES "false"')) 'actual generated NSIS policy define; explicit uninstall is separate'
    Add-Check 'data-removal-is-selectable' ($templateText.Contains('Var DeleteAppDataCheckbox') -and $templateText.Contains('$DeleteAppDataCheckboxState = 1')) 'static template branch only; checkbox behavior remains a runtime test'
    Add-Check 'update-preserves-data-and-run-branches' ($templateText.Contains('${If} $UpdateMode <> 1') -and $templateText.Contains('${AndIf} $UpdateMode <> 1') -and $templateText.Contains('DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"')) 'static branch presence; not an upgrade execution receipt'
    Add-Check 'data-removal-scoped-to-bundle-id' ($templateText.Contains('RmDir /r "$LOCALAPPDATA\${BUNDLEID}"') -and $templateText.Contains('RmDir /r "$APPDATA\${BUNDLEID}"')) 'no custom removal of Codex data or environment-overridden data directories'
    $webviewFailures = [regex]::Matches($activeTemplate, '(?m)^\s*SetErrorLevel\s+\$\{TM_EXIT_WEBVIEW_FAILED\}\s*\r?\n\s*Quit\s*$')
    Add-Check 'missing-webview-failure-aborts' ($activeTemplate.Contains('NSISdl::download_quiet') -and $webviewFailures.Count -eq 3 -and $activeEngine -match '(?m)^!define\s+TM_EXIT_WEBVIEW_FAILED\s+69\s*$') 'actual generated template download/bootstrapper/update failure branches exit 69 and Quit; included engine define, not old Abort text'
    $installerInit = [regex]::Match($activeTemplate, '(?ms)^Function\s+\.onInit\s*\r?\n(.*?)^FunctionEnd\s*$').Groups[1].Value.Trim()
    $silentMacro = [regex]::Match($activeEngine, '(?ms)^!macro\s+TM_ENGINE_REQUIRE_SILENT\s*\r?\n(.*?)^!macroend\s*$').Groups[1].Value.Trim()
    Add-Check 'installer-requires-silent-before-init' ($installerInit -match '^!insertmacro\s+TM_ENGINE_REQUIRE_SILENT\s*(?:\r?\n|$)' -and $silentMacro -match '^\$\{IfNot\}\s+\$\{Silent\}\s*\r?\n\s*SetErrorLevel\s+\$\{TM_EXIT_SILENT_REQUIRED\}\s*\r?\n\s*Quit\s*\r?\n\s*\$\{EndIf\}$' -and $activeEngine -match '(?m)^!define\s+TM_EXIT_SILENT_REQUIRED\s+64\s*$') 'actual installer .onInit first instruction invokes the included non-/S exit-64 gate; no engine execution performed'
    Add-Check 'no-active-force-close-paths' ($activeSources -notmatch '(?im)\b(?:RmShutdown|RmForceShutdown|TerminateProcess|taskkill)\b|::(?:_KillProcess|KillProcess)\b|^\s*!insertmacro\s+CheckIfAppIsRunning\b' -and $activeEngine.Contains('RSTRTMGR::RmGetList') -and ([regex]::Matches($activeTemplate, '(?m)^\s*!insertmacro\s+TM_ENGINE_REQUIRE_APP_STOPPED\s+').Count -eq 3)) 'actual generated template and its included hooks/engine have query-only process guards; unused stock utils definitions do not establish an active call'
    $legacyCallback = [regex]::Match($activeTemplate, '(?ms)^Function\s+PageLeaveReinstall\s*\r?\n(.*?)^FunctionEnd\s*$').Groups[1].Value.Trim()
    $execWaitLines = @([regex]::Matches($activeTemplate, '(?m)^\s*ExecWait\s+([^\r\n]+)') | ForEach-Object { $_.Groups[1].Value.Trim() })
    $allowedExecWait = @('''"$6" ${WEBVIEW2INSTALLERARGS} /install'' $1', '`"$R1" /install appguid=${WEBVIEW2APPGUID}&needsadmin=true` $1')
    # Check the actual generated instruction blocks, including executable quotes,
    # clearing a stale flag and rejecting both launch errors and nonzero exits.
    $webviewExecPrefix = '(?m)^\s*ClearErrors\s*\r?\n\s*ExecWait\s+'
    $webviewExecGate = '\s*\r?\n\s*\$\{IfNot\}\s+\$\{Errors\}\s*\r?\n\s*\$\{AndIf\}\s+\$1\s+=\s+0\s*\r?\n\s*DetailPrint\s+"\$\(webview2InstallSuccess\)"\s*\r?\n\s*\$\{Else\}\s*\r?\n(?:\s*DetailPrint\s+"\$\(webview2InstallError\)"\s*\r?\n)?\s*SetErrorLevel\s+\$\{TM_EXIT_WEBVIEW_FAILED\}\s*\r?\n\s*Quit\s*\r?\n\s*\$\{EndIf\}\s*$'
    Add-Check 'generated-webview-bootstrapper-execwait-error-gate' (([regex]::Matches($activeTemplate, $webviewExecPrefix + [regex]::Escape($allowedExecWait[0]) + $webviewExecGate)).Count -eq 1) 'actual generated bootstrapper block quotes its executable, clears Errors and accepts only no launch error plus exit 0; all other outcomes exit 69'
    Add-Check 'generated-webview-updater-execwait-error-gate' (([regex]::Matches($activeTemplate, $webviewExecPrefix + [regex]::Escape($allowedExecWait[1]) + $webviewExecGate)).Count -eq 1) 'actual generated updater block quotes its executable, clears Errors and accepts only no launch error plus exit 0; all other outcomes exit 69'
    Add-Check 'legacy-uninstaller-execwait-refused' ($legacyCallback -match '^SetErrorLevel\s+\$\{TM_EXIT_UNSUPPORTED_UPGRADE\}\s*\r?\n\s*Quit$' -and $activeEngine -match '(?m)^!define\s+TM_EXIT_UNSUPPORTED_UPGRADE\s+67\s*$' -and $execWaitLines.Count -eq 2 -and @($execWaitLines | Where-Object { $_ -cnotin $allowedExecWait }).Count -eq 0) 'reinstall callback exits 67; the only two active ExecWait commands are the explicit Microsoft WebView2 bootstrapper/updater, never an old uninstall command'
    $uninstallInit = [regex]::Match($activeTemplate, '(?ms)^Function\s+un\.onInit\s*\r?\n(.*?)^FunctionEnd\s*$').Groups[1].Value
    Add-Check 'ordinary-uninstall-ui-retained' ($uninstallInit -and $uninstallInit -notmatch 'TM_ENGINE_REQUIRE_SILENT' -and $activeTemplate.Contains('!insertmacro MUI_UNPAGE_INSTFILES')) 'the silent-only gate is installation-only; ordinary uninstall UI and its existing data choice remain available'
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
    generatedSupportingSources = $generatedSupportingSources
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
