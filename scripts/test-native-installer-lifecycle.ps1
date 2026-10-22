#requires -Version 7.0
param([Parameter(Mandatory)][string]$CandidateDirectory)
$ErrorActionPreference='Stop'
$taskRoot=Split-Path -Parent $PSScriptRoot
$taskCandidate=[IO.Path]::GetFullPath($CandidateDirectory)
$taskReceipt=Get-Content -LiteralPath (Join-Path $taskCandidate 'release/receipt.json') -Raw|ConvertFrom-Json
$taskOldReceipt=Get-Content -LiteralPath (Join-Path $taskRoot 'output/build-candidates/c42-current-v1-20261019/release/receipt.json') -Raw|ConvertFrom-Json
$taskOutput=Join-Path $taskRoot ('output/native-installer-lifecycle-'+(Get-Date -Format 'yyyyMMdd-HHmmss'))
$taskTarget=Join-Path $taskOutput 'app with spaces'
$taskUninstall='Software\Microsoft\Windows\CurrentVersion\Uninstall\TokenMeow'
$taskPreferences='Software\tokenmeow\TokenMeow'
$taskRegistry=[Microsoft.Win32.RegistryKey]::OpenBaseKey('CurrentUser','Registry64')
foreach($taskHiveName in @('CurrentUser','LocalMachine')){
    foreach($taskView in @('Registry32','Registry64')){
        $taskHive=[Microsoft.Win32.RegistryKey]::OpenBaseKey($taskHiveName,$taskView)
        try{$taskKey=$taskHive.OpenSubKey($taskUninstall);if($taskKey){$taskKey.Dispose();throw 'An existing product installation must not be modified by this test.'}}finally{$taskHive.Dispose()}
    }
}
$taskExistingPreference=$taskRegistry.OpenSubKey($taskPreferences)
if($taskExistingPreference){$taskExistingPreference.Dispose();throw 'Existing installer preferences must be preserved; use a clean test profile.'}
if(@(Get-Process -Name token-meow -ErrorAction SilentlyContinue).Count){throw 'A running TokenMeow must not be disturbed.'}
if(-not(Test-Path -LiteralPath 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}')){throw 'This bounded test requires an already installed WebView2; it must not download system components.'}
foreach($taskBinding in @($taskReceipt.engine,$taskOldReceipt.engine)){
    if((Get-FileHash -LiteralPath $taskBinding.path -Algorithm SHA256).Hash -ne $taskBinding.sha256){throw 'Pinned engine hash differs.'}
}
if(Test-Path -LiteralPath $taskOutput){throw 'Preserve previous test evidence.'}
New-Item -ItemType Directory -Path $taskOutput|Out-Null
$taskFramework=Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319'
$taskSource=Join-Path $taskRoot 'installer/TokenMeow.Setup'
$taskHarness=Join-Path $taskOutput 'harness'
New-Item -ItemType Directory -Path $taskHarness|Out-Null
$taskDriver=Join-Path $taskHarness 'NativeLifecycleTests.exe'
$taskBuildInfo=Join-Path $taskCandidate 'work/outer/generated/BuildInfo.cs'
$taskRefs=@('System.dll','System.Core.dll','System.Security.dll','System.Xml.dll','System.Xaml.dll')+@('WindowsBase.dll','PresentationCore.dll','PresentationFramework.dll'|ForEach-Object{Join-Path $taskFramework ('WPF/'+$_)})
$taskArgs=@('/nologo','/platform:x64','/target:exe','/main:NativeLifecycleTests','/utf8output','/codepage:65001',('/out:'+$taskDriver),('/resource:'+$taskReceipt.engine.path+',TokenMeow.Engine.exe'))+@($taskRefs|ForEach-Object{'/reference:'+$_})+@(Get-ChildItem -LiteralPath $taskSource -Filter '*.cs' -File|Select-Object -ExpandProperty FullName)+@((Join-Path $taskSource 'tests/NativeLifecycleTests.cs'),$taskBuildInfo)
& (Join-Path $taskFramework 'csc.exe') @taskArgs > (Join-Path $taskOutput 'compile.log') 2>&1
if($LASTEXITCODE -ne 0){Get-Content (Join-Path $taskOutput 'compile.log');throw 'Native service harness compilation failed.'}
function Invoke-Owned([string]$Exe,[string]$Arguments,[string]$Name){
    $taskStart=[Diagnostics.ProcessStartInfo]::new($Exe,$Arguments);$taskStart.UseShellExecute=$false;$taskStart.CreateNoWindow=$true
    $taskStart.RedirectStandardOutput=$true;$taskStart.RedirectStandardError=$true
    $taskProcess=[Diagnostics.Process]::new();$taskProcess.StartInfo=$taskStart
    try{
        if(-not $taskProcess.Start()){throw 'Could not start owned test process.'}
        $taskOutRead=$taskProcess.StandardOutput.ReadToEndAsync();$taskErrRead=$taskProcess.StandardError.ReadToEndAsync()
        if(-not $taskProcess.WaitForExit(120000)){throw 'Owned test timed out; process preserved for investigation.'}
        $taskOutRead.GetAwaiter().GetResult()|Set-Content -LiteralPath (Join-Path $taskOutput ($Name+'.stdout.log'))
        $taskErrRead.GetAwaiter().GetResult()|Set-Content -LiteralPath (Join-Path $taskOutput ($Name+'.stderr.log'))
        return $taskProcess.ExitCode
    }finally{$taskProcess.Dispose()}
}
function Installed-Version {
    $taskKey=$taskRegistry.OpenSubKey($taskUninstall)
    try{if($taskKey){return [string]$taskKey.GetValue('DisplayVersion')}}finally{if($taskKey){$taskKey.Dispose()}}
}
function App-Hash {return (Get-FileHash -LiteralPath (Join-Path $taskTarget 'token-meow.exe') -Algorithm SHA256).Hash}
function File-Inventory {
    $taskRows=@(Get-ChildItem -LiteralPath $taskTarget -File -Recurse|Sort-Object FullName|ForEach-Object{[pscustomobject]@{name=$_.FullName.Substring($taskTarget.Length+1);sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash}})
    return ($taskRows|ConvertTo-Json -Compress)
}
$taskResults=[Collections.Generic.List[object]]::new()
try{
    $taskFresh=Join-Path $taskOutput 'fresh retry with spaces'
    if((Invoke-Owned $taskDriver ('fail "'+$taskFresh+'"') 'fresh-install-failure') -ne 0 -or (Installed-Version) -or @(Get-ChildItem -LiteralPath $taskFresh -Force).Count -ne 0){throw 'Fresh installation failure did not return to an empty retryable destination.'}
    $taskResults.Add(@{case='actual fresh installation failure restored empty destination and no registration';passed=$true})
    if((Invoke-Owned $taskDriver ('install "'+$taskFresh+'"') 'fresh-install-retry') -ne 0 -or (Installed-Version) -ne $taskReceipt.productVersion){throw 'Fresh installation retry failed.'}
    $taskResults.Add(@{case='actual fresh installation retry succeeded';passed=$true})
    if((Invoke-Owned (Join-Path $taskFresh 'uninstall.exe') ('/S _?='+$taskFresh) 'fresh-uninstall') -ne 0 -or (Installed-Version)){throw 'Fresh retry fixture uninstall failed.'}
    if((Invoke-Owned $taskOldReceipt.engine.path ('/S /NS /D='+$taskTarget) 'old-install') -ne 0 -or (Installed-Version) -ne '0.1.0'){throw 'Actual old candidate installation failed.'}
    $taskBefore=App-Hash
    $taskBeforeFiles=File-Inventory
    $taskBeforeFiles|Set-Content -LiteralPath (Join-Path $taskOutput 'old-files.json')
    $taskResults.Add(@{case='C42 actual 0.1.0 engine installed in owned directory';passed=$true})
    if((Invoke-Owned $taskDriver ('fail "'+$taskTarget+'"') 'upgrade-failure') -ne 0 -or (Installed-Version) -ne '0.1.0' -or (App-Hash) -ne $taskBefore -or (File-Inventory) -cne $taskBeforeFiles){throw 'Actual failed upgrade did not restore old version and complete file inventory.'}
    $taskResults.Add(@{case='new real engine ran, injected failure restored C42 bytes and version';passed=$true})
    if((Invoke-Owned $taskDriver ('install "'+$taskTarget+'"') 'upgrade-success') -ne 0 -or (Installed-Version) -ne $taskReceipt.productVersion -or (App-Hash) -ne $taskReceipt.actualEnginePayload.sha256){throw 'Actual service/engine upgrade proof failed.'}
    $taskResults.Add(@{case='0.1.0 to 0.1.1 actual service/engine upgrade';passed=$true})
    if((Invoke-Owned $taskOldReceipt.engine.path ('/S /NS /D='+$taskTarget) 'downgrade-refused') -ne 68 -or (App-Hash) -ne $taskReceipt.actualEnginePayload.sha256){throw 'Downgrade was not refused.'}
    $taskResults.Add(@{case='actual old engine downgrade refused with unchanged application';passed=$true})
    if((Invoke-Owned $taskDriver ('install "'+$taskTarget+'"') 'reinstall') -ne 0){throw 'Same-version reinstall failed.'}
    $taskResults.Add(@{case='same-version actual service reinstall';passed=$true})
    if((Invoke-Owned (Join-Path $taskTarget 'uninstall.exe') ('/S _?='+$taskTarget) 'uninstall') -ne 0 -or (Installed-Version) -or (Test-Path -LiteralPath (Join-Path $taskTarget 'token-meow.exe'))){throw 'Actual ordinary uninstall failed.'}
    $taskResults.Add(@{case='actual ordinary uninstall removed application and product registration';passed=$true})
    [pscustomobject]@{status='passed';candidate=$taskCandidate;cases=$taskResults;qualified='actual engines and production service via headless harness; final WPF UI and selected data deletion not covered';applicationLaunched=$false;ledgerMigrationAccepted=$false}|ConvertTo-Json -Depth 5|Tee-Object -FilePath (Join-Path $taskOutput 'result.json')
} finally {
    # Remove only the installer preference key created by this test, after successful uninstallation.
    if(-not (Installed-Version)){
        $taskPrefs=$taskRegistry.OpenSubKey($taskPreferences)
        if($taskPrefs){try{$taskOwned=([string]$taskPrefs.GetValue('') -eq $taskTarget) -and $taskPrefs.GetSubKeyNames().Count -eq 0}finally{$taskPrefs.Dispose()};if($taskOwned){$taskRegistry.DeleteSubKey($taskPreferences,$false)}}
    }
    $taskRegistry.Dispose()
}
