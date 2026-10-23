# Preparation by default. -Launch is reserved for the root-approved Native attempt.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Executable,
    [Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$ExpectedSha256,
    [Parameter(Mandatory)][string]$AttemptRoot,
    [ValidateRange(0,65535)][int]$CdpPort=0,
    [ValidateRange(30,600)][int]$OwnerTimeoutSeconds=180,
    [switch]$Launch
)
$ErrorActionPreference='Stop'
function Assert-OrdinaryChain([string]$Path) {
    $subscriptionCursor=[IO.Path]::GetFullPath($Path)
    while($subscriptionCursor){
        if(Test-Path -LiteralPath $subscriptionCursor){if((Get-Item -LiteralPath $subscriptionCursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Reparse points are not accepted'}}
        $subscriptionParent=Split-Path -Parent $subscriptionCursor
        if($subscriptionParent -eq $subscriptionCursor){break}
        $subscriptionCursor=$subscriptionParent
    }
}
function Write-NewJson([string]$Path,$Value) {
    $subscriptionBytes=[Text.UTF8Encoding]::new($false).GetBytes(($Value|ConvertTo-Json -Depth 12)+"`n")
    $subscriptionStream=[IO.File]::Open($Path,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read)
    try{$subscriptionStream.Write($subscriptionBytes)}finally{$subscriptionStream.Dispose()}
}
$subscriptionWorkspace=[IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$subscriptionOutput=[IO.Path]::GetFullPath((Join-Path $subscriptionWorkspace 'output/validation')).TrimEnd('\')
$subscriptionRoot=[IO.Path]::GetFullPath($AttemptRoot).TrimEnd('\')
if(-not $subscriptionRoot.StartsWith($subscriptionOutput+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Attempt root must be fresh and inside workspace output/validation'}
Assert-OrdinaryChain $subscriptionRoot
if(Test-Path -LiteralPath $subscriptionRoot){throw 'Preserve earlier attempts; use a new directory'}
$subscriptionExe=(Resolve-Path -LiteralPath $Executable).Path
Assert-OrdinaryChain $subscriptionExe
if([IO.Path]::GetFileName($subscriptionExe) -ine 'token-meow.exe'){throw 'Select the exact newly built token-meow.exe, not an installer or renamed payload'}
if((Get-FileHash -LiteralPath $subscriptionExe -Algorithm SHA256).Hash -ine $ExpectedSha256){throw 'Executable digest differs'}
if(@(Get-Process -Name token-meow,token_meow -ErrorAction SilentlyContinue).Count){throw 'Existing product process found; do not trigger single-instance handoff'}
if($CdpPort -eq 0){$subscriptionListener=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0)}else{if($CdpPort -lt 1024){throw 'CDP port must be at least 1024'};$subscriptionListener=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,$CdpPort)}
try{$subscriptionListener.Start();$subscriptionPort=$subscriptionListener.LocalEndpoint.Port}finally{$subscriptionListener.Stop()}
$subscriptionSource=Join-Path $subscriptionRoot 'source';$subscriptionData=Join-Path $subscriptionRoot 'data'
foreach($subscriptionRelative in @('source/sessions','data','bin','profile/AppData/Local','profile/AppData/Roaming','temp','webview')){[IO.Directory]::CreateDirectory((Join-Path $subscriptionRoot $subscriptionRelative))|Out-Null}
$subscriptionId=[Guid]::NewGuid().ToString('N')
$subscriptionThread='00000000-0000-4000-8000-000000000001'
$subscriptionFixture=Join-Path $subscriptionSource 'sessions/subscription-native-audit.jsonl'
$subscriptionOld=[DateTime]::UtcNow.AddHours(-1).ToString('o')
$subscriptionRows=@(
    @{timestamp=$subscriptionOld;type='session_meta';payload=@{id=$subscriptionThread;cli_version='0.147.0';cwd=(Join-Path $subscriptionSource 'synthetic-work')}},
    @{timestamp=$subscriptionOld;type='turn_context';payload=@{turn_id='subscription-native-audit-turn';root_turn_id='subscription-native-audit-turn';model='subscription-native-audit-model'}}
)
[IO.File]::WriteAllText($subscriptionFixture,(($subscriptionRows|ForEach-Object{$_|ConvertTo-Json -Depth 5 -Compress}) -join "`n")+"`n",[Text.UTF8Encoding]::new($false))
Write-NewJson (Join-Path $subscriptionRoot 'subscription-native-audit.marker.json') @{schemaVersion=1;kind='subscription-native-audit';attemptId=$subscriptionId;allowNativePlanDelivery=$true;sourceRoot=$subscriptionSource;dataDirectory=$subscriptionData}
$subscriptionWindows=[Environment]::GetFolderPath([Environment+SpecialFolder]::Windows)
$subscriptionPaths=@((Join-Path $subscriptionRoot 'bin'),(Join-Path $subscriptionWindows 'System32'),$subscriptionWindows)
foreach($subscriptionPath in $subscriptionPaths){if(Test-Path -LiteralPath (Join-Path $subscriptionPath 'codex.exe')){throw 'A Codex executable is reachable; refuse launch'}}
$subscriptionLocal=Join-Path $subscriptionRoot 'profile/AppData/Local'
if(Test-Path -LiteralPath (Join-Path $subscriptionLocal 'Programs/OpenAI/Codex/bin/codex.exe')){throw 'Private profile unexpectedly contains Codex'}
$subscriptionEnvironment=[Collections.Generic.Dictionary[string,string]]::new([StringComparer]::OrdinalIgnoreCase)
foreach($subscriptionKey in @('OS','NUMBER_OF_PROCESSORS','PROCESSOR_ARCHITECTURE','PROCESSOR_IDENTIFIER','PROCESSOR_LEVEL','PROCESSOR_REVISION','ProgramFiles','ProgramFiles(x86)','ProgramW6432')){
    $subscriptionValue=[Environment]::GetEnvironmentVariable($subscriptionKey,'Process')
    if(-not [string]::IsNullOrEmpty($subscriptionValue)){$subscriptionEnvironment[$subscriptionKey]=$subscriptionValue}
}
$subscriptionEnvironment['SystemRoot']=$subscriptionWindows;$subscriptionEnvironment['WINDIR']=$subscriptionWindows
$subscriptionEnvironment['SystemDrive']=[IO.Path]::GetPathRoot($subscriptionWindows).TrimEnd('\');$subscriptionEnvironment['COMSPEC']=Join-Path $subscriptionWindows 'System32/cmd.exe'
$subscriptionEnvironment['PATH']=$subscriptionPaths -join [IO.Path]::PathSeparator
$subscriptionEnvironment['USERPROFILE']=Join-Path $subscriptionRoot 'profile';$subscriptionEnvironment['LOCALAPPDATA']=$subscriptionLocal;$subscriptionEnvironment['APPDATA']=Join-Path $subscriptionRoot 'profile/AppData/Roaming'
$subscriptionEnvironment['TEMP']=Join-Path $subscriptionRoot 'temp';$subscriptionEnvironment['TMP']=$subscriptionEnvironment['TEMP']
$subscriptionEnvironment['USERNAME']='SubscriptionAudit';$subscriptionEnvironment['USERDOMAIN']='SubscriptionAudit'
$subscriptionEnvironment['CODEX_HOME']=$subscriptionSource;$subscriptionEnvironment['TOKEN_MEOW_DATA_DIR']=$subscriptionData
$subscriptionEnvironment['WEBVIEW2_USER_DATA_FOLDER']=Join-Path $subscriptionRoot 'webview'
$subscriptionEnvironment['TOKEN_MEOW_DEBUG_CDP_PORT']=[string]$subscriptionPort
$subscriptionEnvironment['TOKEN_MEOW_SUBSCRIPTION_AUDIT_ROOT']=$subscriptionRoot
$subscriptionPlan=@{schemaVersion=1;kind='subscription-native-audit-bootstrap';attemptId=$subscriptionId;root=$subscriptionRoot;sourceRoot=$subscriptionSource;dataDirectory=$subscriptionData;fixture=$subscriptionFixture;threadId=$subscriptionThread;turnId='subscription-native-audit-turn';executable=$subscriptionExe;executableSha256=$ExpectedSha256.ToUpperInvariant();cdpPort=$subscriptionPort;cdpEndpoint=('http://127.0.0.1:'+$subscriptionPort);requiredCargoFeature='subscription-native-audit';requiresDebugAssertions=$true;applicationLaunched=$false;environmentKeys=@($subscriptionEnvironment.Keys|Sort-Object);noCodexCandidates=$true;qualified=$false;scope='Synthetic usage fixture; actual Native IPC/COM/pet evidence must be collected separately; excludes full G3/install/live account'}
Write-NewJson (Join-Path $subscriptionRoot 'bootstrap.json') $subscriptionPlan
if(-not $Launch){$subscriptionPlan|ConvertTo-Json -Depth 5;return}
# This block is not executed by preparation. No shell/environment inheritance,
# startup registration, lock/sleep action, or process-name kill is performed.
$subscriptionStart=[Diagnostics.ProcessStartInfo]::new($subscriptionExe)
$subscriptionStart.UseShellExecute=$false;$subscriptionStart.CreateNoWindow=$true;$subscriptionStart.WindowStyle=[Diagnostics.ProcessWindowStyle]::Hidden
$subscriptionStart.WorkingDirectory=Split-Path -Parent $subscriptionExe
$subscriptionStart.RedirectStandardOutput=$true;$subscriptionStart.RedirectStandardError=$true
$subscriptionStart.Environment.Clear()
foreach($subscriptionPair in $subscriptionEnvironment.GetEnumerator()){$subscriptionStart.Environment[$subscriptionPair.Key]=$subscriptionPair.Value}
$subscriptionProcess=[Diagnostics.Process]::new();$subscriptionProcess.StartInfo=$subscriptionStart
try{
    if(-not $subscriptionProcess.Start()){throw 'Owned application did not start'}
    $subscriptionStdout=$subscriptionProcess.StandardOutput.ReadToEndAsync();$subscriptionStderr=$subscriptionProcess.StandardError.ReadToEndAsync()
    Write-NewJson (Join-Path $subscriptionRoot 'launch.json') @{schemaVersion=1;attemptId=$subscriptionId;processId=$subscriptionProcess.Id;processStartedUtc=$subscriptionProcess.StartTime.ToUniversalTime().ToString('o');executable=$subscriptionExe;executableSha256=$ExpectedSha256.ToUpperInvariant();dataDirectory=$subscriptionData;sourceRoot=$subscriptionSource;cdpEndpoint=$subscriptionPlan.cdpEndpoint;applicationLaunched=$true;qualified=$false}
    Write-Output ('Owned Native attempt started. Inspect '+(Join-Path $subscriptionRoot 'launch.json'))
    if(-not $subscriptionProcess.WaitForExit($OwnerTimeoutSeconds*1000)){Write-NewJson (Join-Path $subscriptionRoot 'owner-timeout.json') @{processId=$subscriptionProcess.Id;status='timeout-unqualified';applicationMayStillBeRunning=$true};throw 'Owned application has not exited; no automatic kill was issued'}
    [IO.File]::WriteAllText((Join-Path $subscriptionRoot 'app-stdout.log'),$subscriptionStdout.GetAwaiter().GetResult(),[Text.UTF8Encoding]::new($false))
    [IO.File]::WriteAllText((Join-Path $subscriptionRoot 'app-stderr.log'),$subscriptionStderr.GetAwaiter().GetResult(),[Text.UTF8Encoding]::new($false))
    Write-NewJson (Join-Path $subscriptionRoot 'exit.json') @{processId=$subscriptionProcess.Id;exitCode=$subscriptionProcess.ExitCode;observedAtUtc=[DateTime]::UtcNow.ToString('o');status='owned-process-exited';fullJobExitQualified=$false}
}finally{$subscriptionProcess.Dispose()}
