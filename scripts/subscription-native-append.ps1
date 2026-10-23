[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$AttemptRoot,
    [Parameter(Mandatory)][ValidateSet(100,900)][int]$Tokens,
    [ValidateRange(0,10)][int]$AgeSeconds=0
)
$ErrorActionPreference='Stop'
$subscriptionRoot=(Resolve-Path -LiteralPath $AttemptRoot).Path
$subscriptionOutput=[IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $PSScriptRoot) 'output/validation')).TrimEnd('\')
if(-not $subscriptionRoot.StartsWith($subscriptionOutput+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Not an owned workspace Native attempt'}
$subscriptionMarker=Get-Content -LiteralPath (Join-Path $subscriptionRoot 'subscription-native-audit.marker.json') -Raw | ConvertFrom-Json
$subscriptionPlan=Get-Content -LiteralPath (Join-Path $subscriptionRoot 'bootstrap.json') -Raw | ConvertFrom-Json
$subscriptionLaunch=Get-Content -LiteralPath (Join-Path $subscriptionRoot 'launch.json') -Raw | ConvertFrom-Json
if($subscriptionMarker.kind -ne 'subscription-native-audit' -or $subscriptionMarker.attemptId -ne $subscriptionPlan.attemptId -or $subscriptionLaunch.attemptId -ne $subscriptionPlan.attemptId){throw 'Attempt binding differs'}
$subscriptionFixture=[IO.Path]::GetFullPath($subscriptionPlan.fixture)
if($subscriptionFixture -ine [IO.Path]::GetFullPath((Join-Path $subscriptionRoot 'source/sessions/subscription-native-audit.jsonl'))){throw 'Unexpected fixture path'}
foreach($subscriptionPath in @($subscriptionRoot,(Join-Path $subscriptionRoot 'source'),(Join-Path $subscriptionRoot 'source/sessions'),$subscriptionFixture)){if((Get-Item -LiteralPath $subscriptionPath -Force).Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Reparse point rejected'}}
$subscriptionProcess=Get-Process -Id $subscriptionLaunch.processId -ErrorAction Stop
if($subscriptionProcess.Path -ine $subscriptionLaunch.executable -or $subscriptionProcess.StartTime.ToUniversalTime().Ticks -ne ([DateTimeOffset]$subscriptionLaunch.processStartedUtc).UtcDateTime.Ticks){throw 'Owned process identity changed'}
$subscriptionNow=[DateTime]::UtcNow
$subscriptionEvent=$subscriptionNow.AddSeconds(-$AgeSeconds)
if($subscriptionEvent -le [DateTime]::Parse($subscriptionLaunch.processStartedUtc).ToUniversalTime()){throw 'Fixture event must be later than this runtime start'}
$subscriptionResponse='native-audit-'+[Guid]::NewGuid().ToString('N')
$subscriptionRow=@{timestamp=$subscriptionEvent.ToString('o');type='token_usage_record';payload=@{thread_id=$subscriptionPlan.threadId;turn_id=$subscriptionPlan.turnId;root_turn_id=$subscriptionPlan.turnId;response_id=$subscriptionResponse;usage=@{input_tokens=$Tokens;output_tokens=0;total_tokens=$Tokens;cached_input_tokens=0;cache_write_input_tokens=0;reasoning_output_tokens=0}}}
$subscriptionBefore=(Get-FileHash -LiteralPath $subscriptionFixture -Algorithm SHA256).Hash
[IO.File]::AppendAllText($subscriptionFixture,($subscriptionRow|ConvertTo-Json -Depth 6 -Compress)+"`n",[Text.UTF8Encoding]::new($false))
$subscriptionReceipt=@{kind='synthetic-native-usage-append';attemptId=$subscriptionPlan.attemptId;appendedAt=$subscriptionNow.ToString('o');eventTime=$subscriptionEvent.ToString('o');tokens=$Tokens;ageSeconds=$AgeSeconds;responseId=$subscriptionResponse;fixture=$subscriptionFixture;beforeSha256=$subscriptionBefore;afterSha256=(Get-FileHash -LiteralPath $subscriptionFixture -Algorithm SHA256).Hash;realModelCalled=$false;realAccountRpcCalled=$false;nativeDeliveryQualified=$false}
$subscriptionReceipt|ConvertTo-Json -Depth 5|Tee-Object -FilePath (Join-Path $subscriptionRoot ($subscriptionResponse+'.json'))
