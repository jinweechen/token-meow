param([string]$EvidenceDirectory=(Join-Path $PSScriptRoot '..\output\performance'),[string]$ReuseJobMetricsPath)
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'resource-metrics.ps1')
$petChecks=[Collections.Generic.List[string]]::new()
function Assert-Meow([bool]$Condition,[string]$Message) {if(-not $Condition){throw $Message};$petChecks.Add($Message)}
$petBase=@{Scenario='idle';RequestedSeconds=600;ElapsedSeconds=600.0;AverageCpu=0.5;AveragePrivate=100.0;PeakPrivate=120.0;Completed=$true;RootExited=$false;Contained=$true}
$petGate=Get-MeowResourceGate @petBase
Assert-Meow ($petGate.gate -eq 'passed') 'A complete contained 600-second interval can satisfy the unchanged idle gate.'
$petShort=$petBase.Clone();$petShort.ElapsedSeconds=598
Assert-Meow ((Get-MeowResourceGate @petShort).gate -eq 'not-qualified') '598 seconds cannot substitute for ten minutes.'
$petPartial=$petBase.Clone();$petPartial.Contained=$false
Assert-Meow ((Get-MeowResourceGate @petPartial).gate -eq 'not-qualified-process-coverage') 'PID observations cannot claim full descendant CPU coverage.'
$petBad=$petBase.Clone();$petBad.Errors=@('counter-access-denied')
Assert-Meow ((Get-MeowResourceGate @petBad).gate -eq 'not-qualified') 'Counter errors cannot silently become zero and pass.'
$petBad=$petBase.Clone();$petBad.PeakPrivate=$null;$petBad.Scenario='import'
Assert-Meow ((Get-MeowResourceGate @petBad).gate -eq 'not-qualified') 'Null import memory cannot pass the 600 MiB gate.'
$petBad=$petBase.Clone();$petBad.RootExited=$true;$petBad.Scenario='import'
Assert-Meow ((Get-MeowResourceGate @petBad).gate -eq 'not-qualified') 'An early root exit cannot pass import memory.'
$petBad=$petBase.Clone();$petBad.PeakPrivate=350.001
Assert-Meow ((Get-MeowResourceGate @petBad).gate -eq 'failed') 'Resident memory above 350 MiB still fails.'
$petBad=$petBase.Clone();$petBad.Scenario='import';$petBad.PeakPrivate=600.001
Assert-Meow ((Get-MeowResourceGate @petBad).gate -eq 'failed') 'Import memory above 600 MiB still fails.'
$petBad=$petBase.Clone();$petBad.AverageCpu=1.0001
Assert-Meow ((Get-MeowResourceGate @petBad).gate -eq 'failed') 'Average CPU above 1 percent still fails.'
$petReduced=Get-MeowReducedMetrics -LogicalProcessors 12 -Samples @(
    @{elapsedSeconds=0;totalUser100ns=10000000;totalKernel100ns=10000000;privateBytes=100MB},
    @{elapsedSeconds=1;totalUser100ns=11000000;totalKernel100ns=10000000;privateBytes=200MB},
    @{elapsedSeconds=10;totalUser100ns=22000000;totalKernel100ns=10000000;privateBytes=200MB})
Assert-Meow ([Math]::Abs($petReduced.cpu-1) -lt 0.000001) 'CPU uses cumulative endpoint delta, elapsed seconds, and all twelve logical processors.'
Assert-Meow ([Math]::Abs($petReduced.averagePrivate-195) -lt 0.000001) 'Unequal sample intervals use a time-weighted memory mean.'
if($ReuseJobMetricsPath) {
    $petMetricsPath=(Resolve-Path -LiteralPath $ReuseJobMetricsPath).Path;$petSandbox=Split-Path -Parent $petMetricsPath
    $petReport=Join-Path $petSandbox 'short-child.json';$petFixture=Join-Path $petSandbox 'resource-fixture.exe'
} else {
    $petSandbox=Join-Path ([IO.Path]::GetFullPath($EvidenceDirectory)) ('observer-test-'+[Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $petSandbox -Force | Out-Null
    $petFixture=Join-Path $petSandbox 'resource-fixture.exe';$petReport=Join-Path $petSandbox 'short-child.json';$petMetricsPath=Join-Path $petSandbox 'job.samples.jsonl'
    & rustc --edition 2024 -O -o $petFixture (Join-Path $PSScriptRoot 'resource-fixture.rs')
    if($LASTEXITCODE -ne 0){throw 'Fixture compilation failed.'}
    $null=& (Join-Path $PSScriptRoot 'start-resource-job.ps1') -Executable $petFixture -ArgumentList @($petReport) -Fixture -DurationSeconds 10 -OutputPath $petMetricsPath
}
$petJob=Read-MeowJobEvidence -Path $petMetricsPath -RequestedSeconds 10
if(-not $ReuseJobMetricsPath) {
    $petStillRunning=Get-Process -Id $petJob.start.rootProcessId -ErrorAction SilentlyContinue
    Assert-Meow ($petStillRunning -and $petStillRunning.StartTime.ToUniversalTime().ToFileTimeUtc() -eq $petJob.start.rootCreationFileTime) 'Closing the observer leaves the already running fixture alive.'
}
$petChild=Get-Content -Raw -LiteralPath $petReport | ConvertFrom-Json
$petParent=Get-Content -Raw -LiteralPath ($petReport+'.parent.json') | ConvertFrom-Json
Assert-Meow ($petParent.nestedJob -eq $true) 'A nested application Job remains inside the observer Job.'
$petChildObserved=@($petJob.samples | ForEach-Object {$_.processes} | Where-Object {$_.id -eq $petChild.childProcessId}).Count -gt 0
Assert-Meow (-not $petChildObserved -and $petJob.samples[-1].totalProcesses -gt $petJob.samples[0].totalProcesses -and $petChild.childWallSeconds -lt 1) 'The CPU child starts and exits wholly between one-second samples.'
Assert-Meow ($petJob.metrics.cpu100ns -ge $petChild.childCpu100ns-320000) 'Job cumulative CPU retains the exited child CPU within Windows accounting precision.'
Assert-Meow ($petJob.metrics.elapsed -ge 10 -and $petJob.errors.Count -eq 0) 'The real Job fixture covers the full requested window without counter errors.'
$petSummary=(& (Join-Path $PSScriptRoot 'measure-resources.ps1') -JobMetricsPath $petMetricsPath -DurationSeconds 10 -Scenario idle -EvidenceDirectory $petSandbox) | ConvertFrom-Json
Assert-Meow ($petSummary.gate -eq 'fixture-only' -and $petSummary.thresholdGate -eq 'not-qualified') 'A short fixture run never claims the application idle release gate.'
$petLines=@([IO.File]::ReadAllLines($petMetricsPath));$petTampered=Join-Path $petSandbox 'recycled-job.jsonl'
$petSecond=$petLines[2] | ConvertFrom-Json;$petSecond.jobId=[Guid]::NewGuid().ToString('D');$petLines[2]=$petSecond | ConvertTo-Json -Compress -Depth 8
[IO.File]::WriteAllLines($petTampered,$petLines,[Text.UTF8Encoding]::new($false))
$petRejected=$false;try{$null=Read-MeowJobEvidence -Path $petTampered -RequestedSeconds 10}catch{$petRejected=$true}
Assert-Meow $petRejected 'Mixed Job identities are rejected before aggregation.'
function Assert-MeowRejectedRecord([int]$Index,[scriptblock]$Mutate,[string]$Name) {
    $petMutationLines=@([IO.File]::ReadAllLines($petMetricsPath));$petRecord=$petMutationLines[$Index] | ConvertFrom-Json
    & $Mutate $petRecord
    $petMutationLines[$Index]=$petRecord | ConvertTo-Json -Compress -Depth 8
    $petMutatedFile=Join-Path $petSandbox ($Name+'.jsonl')
    [IO.File]::WriteAllLines($petMutatedFile,$petMutationLines,[Text.UTF8Encoding]::new($false))
    $petWasRejected=$false;try{$null=Read-MeowJobEvidence -Path $petMutatedFile -RequestedSeconds 10}catch{$petWasRejected=$true}
    Assert-Meow $petWasRejected $Name
}
Assert-MeowRejectedRecord 3 {param($record) $record.totalUser100ns=0} 'Cumulative CPU rollback is rejected.'
Assert-MeowRejectedRecord 2 {param($record) $record.elapsedSeconds=0} 'Clock rollback is rejected.'
Assert-MeowRejectedRecord 1 {param($record) $record.PSObject.Properties.Remove('measurementErrors')} 'Missing memory coverage status is rejected.'
Assert-MeowRejectedRecord 0 {param($record) $record.helperOutsideJob='true'} 'Containment guarantees cannot use truthy strings.'
Assert-MeowRejectedRecord ($petLines.Count-1) {param($record) $record.executableSha256After='0'*64} 'Changing executable identity is rejected.'
if(-not ('TokenMeow.Acceptance.ResourceJob' -as [type])) {Add-Type -Path (Join-Path $PSScriptRoot 'resource-job.cs')}
$petUnresumed=[TokenMeow.Acceptance.ResourceJob]::new($petFixture,@($petReport),$petSandbox)
$petUnresumedId=$petUnresumed.RootProcessId;$petUnresumedCreation=$petUnresumed.RootCreationFileTime
Assert-Meow $petUnresumed.ObserverOutsideJob 'Native membership confirms the observer is outside the target Job.'
$petUnresumed.Dispose();Start-Sleep -Milliseconds 100
$petRemaining=Get-Process -Id $petUnresumedId -ErrorAction SilentlyContinue
Assert-Meow (-not $petRemaining -or $petRemaining.StartTime.ToUniversalTime().ToFileTimeUtc() -ne $petUnresumedCreation) 'Failure before Resume does not strand a suspended fixture.'
$petResult=[ordered]@{passed=$petChecks.Count;checks=$petChecks.ToArray();evidenceDirectory=$petSandbox;childCpu100ns=$petChild.childCpu100ns;jobCpuDelta100ns=$petJob.metrics.cpu100ns;observedSeconds=$petJob.metrics.elapsed;applicationWasNotLaunched=$true
    nativeCpuEvidenceReused=[bool]$ReuseJobMetricsPath;nativeCpuEvidenceFile=$petMetricsPath;nativeCpuEvidenceSha256=(Get-FileHash -LiteralPath $petMetricsPath -Algorithm SHA256).Hash}
$petResult | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $petSandbox ('test-summary-'+(Get-Date -Format 'yyyyMMdd-HHmmss-fff')+'.json')) -Encoding utf8
$petResult | ConvertTo-Json -Depth 5
