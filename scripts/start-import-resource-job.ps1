[CmdletBinding(DefaultParameterSetName='Run')]
param(
    [Parameter(Mandatory=$true,ParameterSetName='Run')][string]$Executable,
    [Parameter(Mandatory=$true,ParameterSetName='Run')][string]$ExpectedSha256,
    [Parameter(Mandatory=$true,ParameterSetName='Run')][string]$SourceRoot,
    [Parameter(Mandatory=$true,ParameterSetName='Run')][string]$SourceManifestPath,
    [Parameter(Mandatory=$true,ParameterSetName='Run')][string]$ExpectedSourceManifestSha256,
    [Parameter(Mandatory=$true,ParameterSetName='Run')][string]$DataDirectory,
    [Parameter(Mandatory=$true,ParameterSetName='Run')][string]$PrepareReceiptPath,
    [Parameter(Mandatory=$true,ParameterSetName='Run')][string]$OutputDirectory,
    [Parameter(Mandatory=$true,ParameterSetName='SelfTest')][switch]$SelfTest
)

# Separate full-import protocol. Never interpreted as an idle-600-second receipt.
# Running this file without -SelfTest intentionally launches the selected app.
$ErrorActionPreference='Stop'
$taskPython=(Get-Command python -CommandType Application -ErrorAction Stop|Select-Object -First 1).Source
$taskSidecarPath=Join-Path $PSScriptRoot 'observe-import-completion.py'
if($SelfTest) { & $taskPython -B $taskSidecarPath --self-test; if($LASTEXITCODE -ne 0){throw 'Import observer self-test failed'}; return }
if($PSVersionTable.PSVersion.Major -lt 7){throw 'PowerShell 7 is required for structured helper process arguments'}

function Assert-NoReparse([string]$Path) {
    $taskItem=[IO.FileSystemInfo](Get-Item -LiteralPath $Path -Force)
    while($taskItem) {
        if(($taskItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'Reparse points are not allowed'}
        $taskItem=if($taskItem -is [IO.FileInfo]){$taskItem.Directory}else{$taskItem.Parent}
    }
}
function Write-NewJson([string]$Path,$Value) {
    $taskData=[Text.Encoding]::UTF8.GetBytes(($Value|ConvertTo-Json -Depth 12))
    $taskTemporary=$Path+'.writing-'+[Guid]::NewGuid().ToString('N')
    $taskStream=[IO.File]::Open($taskTemporary,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read)
    try{$taskStream.Write($taskData,0,$taskData.Length);$taskStream.Flush($true)}finally{$taskStream.Dispose()}
    [IO.File]::Move($taskTemporary,$Path)
}
$taskPerformance=[IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $PSScriptRoot) 'output\performance'))
$taskOutput=[IO.Path]::GetFullPath($OutputDirectory)
if(-not $taskOutput.StartsWith($taskPerformance+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw 'Use a fresh workspace output/performance observer directory'}
if(Test-Path -LiteralPath $taskOutput){throw 'Observer output must be a new directory'}
$taskExisting=[IO.DirectoryInfo]::new((Split-Path -Parent $taskOutput))
while(-not $taskExisting.Exists){$taskExisting=$taskExisting.Parent}
Assert-NoReparse $taskExisting.FullName
New-Item -ItemType Directory -Path $taskOutput -ErrorAction Stop|Out-Null
Assert-NoReparse $taskOutput
$taskExe=(Resolve-Path -LiteralPath $Executable).Path
Assert-NoReparse $taskExe
$taskPlan=[ordered]@{
    protocol='full-first-import-v1';maxSeconds=3600;sampleMilliseconds=1000;maximumSampleGapSeconds=2.5
    privatePeakLimitMiB=600;requiredFreshCompleteIterations=3
    executable=$taskExe;expectedSha256=$ExpectedSha256
    sourceRoot=[IO.Path]::GetFullPath($SourceRoot);sourceManifestPath=[IO.Path]::GetFullPath($SourceManifestPath)
    expectedSourceManifestSha256=$ExpectedSourceManifestSha256
    dataDirectory=[IO.Path]::GetFullPath($DataDirectory);prepareReceiptPath=[IO.Path]::GetFullPath($PrepareReceiptPath)
    observerDirectory=$taskOutput;arguments=@('--background');createdAt=[DateTime]::UtcNow.ToString('o')
    completionPolicy='All frozen byte cursors + zero history/pending + 3 distinct fresh collector iterations; coverage gaps cannot qualify'
    failFastPolicy='A valid sampled private-memory counterexample above 600 MiB ends observation; application remains running'
}
$taskPlanPath=Join-Path $taskOutput 'plan.json'
Write-NewJson $taskPlanPath $taskPlan
$taskPreflightText=& $taskPython -B $taskSidecarPath --preflight --plan $taskPlanPath
if($LASTEXITCODE -ne 0){
    Write-NewJson (Join-Path $taskOutput 'summary.json') @{protocol='full-first-import-v1';gate='not-qualified';reason='preflight-failed';applicationLaunched=$false}
    throw 'Full-import preflight failed; no application was launched'
}
$taskPreflight=$taskPreflightText|ConvertFrom-Json
if(-not $taskPreflight.passed){throw 'Full-import preflight did not pass'}
Write-NewJson (Join-Path $taskOutput 'preflight.json') $taskPreflight
if(-not ('TokenMeow.Acceptance.ResourceJob' -as [type])){Add-Type -Path (Join-Path $PSScriptRoot 'resource-job.cs')}
. (Join-Path $PSScriptRoot 'resource-metrics.ps1')
# Metadata and full input hashes are collected before CreateProcess/suspension.
$taskCpu=(Get-CimInstance Win32_Processor|Select-Object -First 1 -ExpandProperty Name)
$taskObserverSha=(Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash
$taskSidecarSha=(Get-FileHash -LiteralPath $taskSidecarPath -Algorithm SHA256).Hash
$taskJobSha=(Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'resource-job.cs') -Algorithm SHA256).Hash
$taskOldSource=[Environment]::GetEnvironmentVariable('CODEX_HOME','Process')
$taskOldData=[Environment]::GetEnvironmentVariable('TOKEN_MEOW_DATA_DIR','Process')
$taskJob=$null;$taskHelper=$null;$taskWriter=$null;$taskResumed=$false
$taskErrors=[Collections.Generic.List[string]]::new()
$taskSamples=[Collections.Generic.List[object]]::new()
$taskReason='observer-error';$taskCompletion=$null;$taskFailedMemory=$false;$taskRootExited=$false
$taskCompletionBindingValid=$false;$taskHelperExitCode=$null
try {
    $taskControlPath=Join-Path $taskOutput 'launch-binding.json'
    $taskStartInfo=[Diagnostics.ProcessStartInfo]::new($taskPython)
    $taskStartInfo.UseShellExecute=$false;$taskStartInfo.CreateNoWindow=$true
    $taskStartInfo.WindowStyle=[Diagnostics.ProcessWindowStyle]::Hidden
    foreach($taskArg in @('-B',$taskSidecarPath,'--observe','--plan',$taskPlanPath,'--control',$taskControlPath)){$taskStartInfo.ArgumentList.Add($taskArg)}
    $taskHelper=[Diagnostics.Process]::Start($taskStartInfo)
    [Environment]::SetEnvironmentVariable('CODEX_HOME',$taskPlan.sourceRoot,'Process')
    [Environment]::SetEnvironmentVariable('TOKEN_MEOW_DATA_DIR',$taskPlan.dataDirectory,'Process')
    $taskJobStream=[IO.File]::Open((Join-Path $taskOutput 'import-job.jsonl'),[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read)
    $taskWriter=[IO.StreamWriter]::new($taskJobStream,[Text.UTF8Encoding]::new($false))
    $taskJob=[TokenMeow.Acceptance.ResourceJob]::new($taskExe,@('--background'),(Split-Path -Parent $taskExe))
    $taskStart=[ordered]@{
        protocol='full-first-import-v1';kind='import-start';jobId=$taskJob.JobId
        rootProcessId=$taskJob.RootProcessId;rootCreationFileTime=$taskJob.RootCreationFileTime
        executablePath=$taskExe;executableSha256=$ExpectedSha256;sourceManifestSha256=$ExpectedSourceManifestSha256
        planSha256=(Get-FileHash -LiteralPath $taskPlanPath -Algorithm SHA256).Hash
        prepareReceiptSha256=$taskPreflight.prepareReceiptSha256;inputFiles=$taskPreflight.inputFiles;inputBytes=$taskPreflight.inputBytes
        sourceId=$taskPreflight.sourceId;importSince=$taskPreflight.importSince;clearWatermark=$taskPreflight.clearWatermark
        assignBeforeResume=$true;breakawayAllowed=$false;helperOutsideJob=$taskJob.ObserverOutsideJob;jobCpuIncludesExited=$true
        logicalProcessors=[TokenMeow.Acceptance.ResourceJob]::LogicalProcessors;cpuName=$taskCpu
        maxSeconds=3600;sampleMilliseconds=1000;maximumSampleGapSeconds=2.5;privatePeakLimitMiB=600
        requiredFreshCompleteIterations=3;baselineBeforeResume=$true;utc=[DateTime]::UtcNow.ToString('o')
        observerSourceSha256=$taskObserverSha;sidecarSha256=$taskSidecarSha;resourceJobSha256=$taskJobSha
    }
    Write-NewJson $taskControlPath $taskStart
    $taskWriter.WriteLine(($taskStart|ConvertTo-Json -Compress -Depth 8));$taskWriter.Flush()
    $taskSample=$taskJob.Sample();$taskSamples.Add($taskSample)
    foreach($taskError in $taskSample.measurementErrors){$taskErrors.Add([string]$taskError)}
    $taskWriter.WriteLine(($taskSample|ConvertTo-Json -Compress -Depth 8));$taskWriter.Flush()
    $taskJob.Resume();$taskResumed=$true
    while($true) {
        $taskPrevious=$taskSample
        Start-Sleep -Milliseconds 1000
        $taskSample=$taskJob.Sample();$taskSamples.Add($taskSample)
        $taskWriter.WriteLine(($taskSample|ConvertTo-Json -Compress -Depth 8));$taskWriter.Flush()
        $taskGap=[double]$taskSample.elapsedSeconds-[double]$taskPrevious.elapsedSeconds
        foreach($taskError in $taskSample.measurementErrors){$taskErrors.Add([string]$taskError)}
        if($taskGap -gt 2.5){$taskErrors.Add('sample-gap-exceeds-2.5-seconds')}
        if($taskSample.rootExited){$taskRootExited=$true;$taskReason='early-process-exit';break}
        if($taskSample.privateBytes -gt 600MB -and @($taskSample.measurementErrors).Count -eq 0 -and $taskGap -le 2.5){
            $taskFailedMemory=$true;$taskReason='failed-observed-import-memory-threshold';break
        }
        if($taskSample.elapsedSeconds -ge 3600){$taskReason='deadline-3600-seconds';break}
        $taskCompletionPath=Join-Path $taskOutput 'completion.json'
        if(Test-Path -LiteralPath $taskCompletionPath){
            $taskCompletion=Get-Content -LiteralPath $taskCompletionPath -Raw|ConvertFrom-Json
            $taskCompletionBindingValid=($taskCompletion.protocol -eq 'full-first-import-v1' -and
                $taskCompletion.jobId -eq $taskJob.JobId -and $taskCompletion.rootProcessId -eq $taskJob.RootProcessId -and
                $taskCompletion.rootCreationFileTime -eq $taskJob.RootCreationFileTime)
            if(-not $taskCompletionBindingValid){$taskErrors.Add('completion-receipt-binding-mismatch');$taskReason='completion-binding-mismatch'}else{$taskReason=[string]$taskCompletion.status}
            break
        }
        if($taskHelper.HasExited){$taskReason='completion-sidecar-exited-without-receipt';break}
    }
} catch {
    $taskErrors.Add($_.Exception.Message)
} finally {
    if(-not (Test-Path -LiteralPath (Join-Path $taskOutput 'stop.json'))){Write-NewJson (Join-Path $taskOutput 'stop.json') @{stop=$true;utc=[DateTime]::UtcNow.ToString('o')}}
    if($taskHelper){
        if(-not $taskHelper.WaitForExit(3000)){
            $taskErrors.Add('sidecar-did-not-stop-within-three-seconds');$taskHelper.Kill();$null=$taskHelper.WaitForExit(1000)
        }
        if($taskHelper.HasExited){$taskHelperExitCode=$taskHelper.ExitCode;if($taskHelperExitCode -ne 0){$taskErrors.Add('sidecar-exit-code-'+$taskHelperExitCode)}}
        $taskHelper.Dispose()
    }
    if($taskJob){$taskJob.Dispose()}
    [Environment]::SetEnvironmentVariable('CODEX_HOME',$taskOldSource,'Process')
    [Environment]::SetEnvironmentVariable('TOKEN_MEOW_DATA_DIR',$taskOldData,'Process')
}
$taskHashAfter=(Get-FileHash -LiteralPath $taskExe -Algorithm SHA256).Hash
if($taskHashAfter -ne $ExpectedSha256){$taskErrors.Add('executable-digest-changed')}
if((Get-FileHash -LiteralPath $taskPlan.sourceManifestPath -Algorithm SHA256).Hash -ne $ExpectedSourceManifestSha256){$taskErrors.Add('source-manifest-digest-changed')}
try{$taskReduced=Get-MeowReducedMetrics -Samples $taskSamples.ToArray() -LogicalProcessors ([TokenMeow.Acceptance.ResourceJob]::LogicalProcessors)}catch{
    $taskErrors.Add($_.Exception.Message)
    $taskReduced=[pscustomobject]@{elapsed=0;peakPrivate=$null;averagePrivate=$null;cpu=$null}
}
$taskCompletionQualified=($null -ne $taskCompletion -and $taskCompletion.completionQualified -is [bool] -and
    $taskCompletion.completionQualified -eq $true -and $taskCompletionBindingValid -and $taskHelperExitCode -eq 0 -and
    $taskCompletion.allFrozenInputHashesVerifiedAfterCompletion -eq $true)
$taskMeasured=($taskResumed -and -not $taskRootExited -and $taskErrors.Count -eq 0 -and $taskSamples.Count -gt 1)
$taskGate=if($taskFailedMemory){'failed-observed-import-memory-threshold'}elseif($taskMeasured -and $taskCompletionQualified -and $taskReduced.peakPrivate -le 600){'passed-full-first-import-sampled-private-memory'}else{'not-qualified'}
$taskSummary=[ordered]@{
    protocol='full-first-import-v1';kind='import-end';gate=$taskGate;reason=$taskReason
    rootProcessId=$(if($taskJob){$taskJob.RootProcessId}else{$null});jobId=$(if($taskJob){$taskJob.JobId}else{$null})
    executableSha256Before=$ExpectedSha256;executableSha256After=$taskHashAfter;sourceManifestSha256=$ExpectedSourceManifestSha256
    completionQualified=$taskCompletionQualified;measurementQualified=$taskMeasured
    completionBindingValid=$taskCompletionBindingValid;sidecarExitCode=$taskHelperExitCode
    observedSeconds=$taskReduced.elapsed;sampleCount=$taskSamples.Count;sampledPeakPrivateMiB=$taskReduced.peakPrivate
    timeWeightedMeanPrivateMiB=$taskReduced.averagePrivate;normalizedMeanCpuPercent=$taskReduced.cpu
    rootExited=$taskRootExited;applicationLaunched=$taskResumed;applicationLeftRunning=($taskResumed -and -not $taskRootExited)
    measurementErrors=@($taskErrors.ToArray());completionReceipt=$taskCompletion
    qualificationScope='Complete first-import sampled private-memory peak only; CPU is descriptive and this is never idle-600s qualification'
    limitations=@('Private memory is sampled at one second, not a continuous maximum.','Timeout/error/threshold failure may leave the first import incomplete.','Closing the observer leaves a resumed application running for explicit normal exit.')
    completedAt=[DateTime]::UtcNow.ToString('o')
}
if($taskWriter){$taskWriter.WriteLine(($taskSummary|ConvertTo-Json -Compress -Depth 12));$taskWriter.Flush();$taskWriter.Dispose()}
Write-NewJson (Join-Path $taskOutput 'summary.json') $taskSummary
$taskSummary|ConvertTo-Json -Depth 12
