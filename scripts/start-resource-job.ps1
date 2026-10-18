param(
    [Parameter(Mandatory=$true)][string]$Executable,
    [string[]]$ArgumentList=@(),
    [string]$WorkingDirectory,
    [ValidateRange(1,7200)][int]$DurationSeconds=600,
    [ValidateRange(0,1800)][int]$WarmupSeconds=0,
    [ValidateRange(100,5000)][int]$SampleMilliseconds=1000,
    [Parameter(Mandatory=$true)][string]$OutputPath,
    [string]$ExpectedSha256,
    [switch]$Fixture
)
$ErrorActionPreference='Stop'
$petExecutable=(Resolve-Path -LiteralPath $Executable).Path
if(-not $Fixture -and [IO.Path]::GetFileNameWithoutExtension($petExecutable) -notin @('token-meow','token_meow','TokenMeow')) { throw 'Only the application or an explicitly labelled fixture may be launched.' }
$petHash=(Get-FileHash -LiteralPath $petExecutable -Algorithm SHA256).Hash
if(-not $Fixture -and -not $ExpectedSha256) { throw 'Application acceptance requires -ExpectedSha256.' }
if($ExpectedSha256 -and $petHash -ne $ExpectedSha256) { throw 'Executable SHA-256 does not match the expected build.' }
if(-not $WorkingDirectory) { $WorkingDirectory=Split-Path -Parent $petExecutable }
$petOutput=[IO.Path]::GetFullPath($OutputPath)
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $petOutput) | Out-Null
if(Test-Path -LiteralPath $petOutput) { throw 'Evidence already exists; choose a new OutputPath.' }
if(-not ('TokenMeow.Acceptance.ResourceJob' -as [type])) { Add-Type -Path (Join-Path $PSScriptRoot 'resource-job.cs') }
# Read observer metadata before creating the suspended application. These queries
# must not inflate the application's external CreateProcess-to-ready upper bound.
$petCpuName=(Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name)
$petOsVersion=[Environment]::OSVersion.Version.ToString()
$petObserverSourceSha256=(Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'resource-job.cs') -Algorithm SHA256).Hash
$petObserverScriptSha256=(Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash
$petWriter=[IO.StreamWriter]::new($petOutput,$false,[Text.UTF8Encoding]::new($false))
$petJob=$null
try {
    $petJob=[TokenMeow.Acceptance.ResourceJob]::new($petExecutable,$ArgumentList,$WorkingDirectory)
    $petStart=[ordered]@{
        schemaVersion=1;kind='start';jobId=$petJob.JobId;rootProcessId=$petJob.RootProcessId
        rootCreationFileTime=$petJob.RootCreationFileTime;executablePath=$petExecutable;executableSha256=$petHash
        assignBeforeResume=$true;breakawayAllowed=$false;helperOutsideJob=$petJob.ObserverOutsideJob;jobCpuIncludesExited=$true
        logicalProcessors=[TokenMeow.Acceptance.ResourceJob]::LogicalProcessors
        requestedSeconds=$DurationSeconds;warmupSeconds=$WarmupSeconds;sampleMilliseconds=$SampleMilliseconds
        isFixture=[bool]$Fixture;utc=[DateTime]::UtcNow.ToString('o')
        osVersion=$petOsVersion;cpuName=$petCpuName
        observerSourceSha256=$petObserverSourceSha256
        observerScriptSha256=$petObserverScriptSha256
    }
    $petWriter.WriteLine(($petStart | ConvertTo-Json -Compress -Depth 8));$petWriter.Flush()
    $petJob.Resume()
    if($WarmupSeconds) { Start-Sleep -Seconds $WarmupSeconds }
    $petBaseline=$petJob.Sample();$petSample=$petBaseline
    $petWriter.WriteLine(($petSample | ConvertTo-Json -Compress -Depth 8));$petWriter.Flush()
    while(-not $petSample.rootExited -and $petSample.elapsedSeconds-$petBaseline.elapsedSeconds -lt $DurationSeconds) {
        $petRemaining=$DurationSeconds-($petSample.elapsedSeconds-$petBaseline.elapsedSeconds)
        Start-Sleep -Milliseconds ([int][Math]::Max(1,[Math]::Min($SampleMilliseconds,[Math]::Ceiling(1000*$petRemaining))))
        $petSample=$petJob.Sample()
        $petWriter.WriteLine(($petSample | ConvertTo-Json -Compress -Depth 8));$petWriter.Flush()
    }
    $petEnd=[ordered]@{kind='end';jobId=$petJob.JobId;elapsedSeconds=$petSample.elapsedSeconds
        rootExited=$petSample.rootExited;completed=(-not $petSample.rootExited -and $petSample.elapsedSeconds-$petBaseline.elapsedSeconds -ge $DurationSeconds)
        measurementErrors=@();executableSha256After=(Get-FileHash -LiteralPath $petExecutable -Algorithm SHA256).Hash
        applicationLeftRunning=(-not $petSample.rootExited);utc=[DateTime]::UtcNow.ToString('o')}
    $petWriter.WriteLine(($petEnd | ConvertTo-Json -Compress -Depth 8));$petWriter.Flush()
    [pscustomobject]@{metricsFile=$petOutput;rootProcessId=$petJob.RootProcessId;completed=$petEnd.completed;applicationLeftRunning=$petEnd.applicationLeftRunning} | ConvertTo-Json
} catch {
    if($petJob) { $petWriter.WriteLine((@{kind='error';jobId=$petJob.JobId;message=$_.Exception.Message} | ConvertTo-Json -Compress)) }
    throw
} finally {
    if($petJob) { $petJob.Dispose() };$petWriter.Dispose()
}
