param(
    [int]$RootProcessId=0,
    [string]$JobMetricsPath,
    [ValidateRange(10,7200)][int]$DurationSeconds=600,
    [ValidateSet('idle','import','lifecycle')][string]$Scenario='idle',
    [string]$EvidenceDirectory=(Join-Path $PSScriptRoot '..\output\performance')
)
$ErrorActionPreference='Stop'
if(($RootProcessId -gt 0) -eq (-not [string]::IsNullOrWhiteSpace($JobMetricsPath))) {throw 'Specify exactly one of -RootProcessId or -JobMetricsPath.'}
. (Join-Path $PSScriptRoot 'resource-metrics.ps1')
$petEvidencePath=[IO.Path]::GetFullPath($EvidenceDirectory)
New-Item -ItemType Directory -Force -Path $petEvidencePath | Out-Null
$petStamp=(Get-Date -Format 'yyyyMMdd-HHmmss-fff')+'-'+[Guid]::NewGuid().ToString('N').Substring(0,8)
$petSamplePath=Join-Path $petEvidencePath "$Scenario-$petStamp.samples.jsonl"
$petSummaryPath=Join-Path $petEvidencePath "$Scenario-$petStamp.summary.json"

if($JobMetricsPath) {
    $petJob=Read-MeowJobEvidence -Path $JobMetricsPath -RequestedSeconds $DurationSeconds
    $petMetrics=$petJob.metrics
    $petGate=Get-MeowResourceGate -Scenario $Scenario -RequestedSeconds $DurationSeconds -ElapsedSeconds $petMetrics.elapsed `
        -AverageCpu $petMetrics.cpu -AveragePrivate $petMetrics.averagePrivate -PeakPrivate $petMetrics.peakPrivate `
        -Completed ([bool]$petJob.end.completed) -RootExited ([bool]$petJob.end.rootExited) -Errors $petJob.errors `
        -Contained $true -Fixture ([bool]$petJob.start.isFixture) -LogicalProcessors $petJob.start.logicalProcessors
    $petSummary=[ordered]@{
        scenario=$Scenario;measurementMode='windows-job';jobId=$petJob.start.jobId
        rootProcessId=$petJob.start.rootProcessId;rootCreationFileTime=$petJob.start.rootCreationFileTime
        rootExecutable=$petJob.start.executablePath;executableSha256=$petJob.start.executableSha256
        observerSourceSha256=$petJob.start.observerSourceSha256;observerScriptSha256=$petJob.start.observerScriptSha256
        logicalProcessors=$petJob.start.logicalProcessors;cpuName=$petJob.start.cpuName;osVersion=$petJob.start.osVersion
        requestedDurationSeconds=$DurationSeconds;observedDurationSeconds=$petMetrics.elapsed;samples=$petJob.samples.Count
        rootExited=$petJob.end.rootExited;cpuAveragePercent=$petMetrics.cpu;cpuDelta100ns=$petMetrics.cpu100ns
        privateAverageMiB=$petMetrics.averagePrivate;privatePeakMiB=$petMetrics.peakPrivate
        jobLifetimePeakCommitMiB=($petJob.samples | Measure-Object jobLifetimePeakCommitBytes -Maximum).Maximum/1MB
        processCoverage='assigned-before-resume-no-breakaway';cpuIncludesExitedProcesses=$true
        initialTotalProcesses=$petJob.samples[0].totalProcesses;finalTotalProcesses=$petJob.samples[-1].totalProcesses
        finalActiveProcesses=$petJob.samples[-1].activeProcesses;measurementErrors=$petJob.errors
        gate=$petGate.gate;thresholdGate=$petGate.thresholdGate;samplesFile=(Resolve-Path -LiteralPath $JobMetricsPath).Path
        qualificationScope='resource-metrics-only'
        gpuCountersAvailable=$false;gpuCounterError='Not collected by the Job observer.'
        limitations=@('Current private memory and working set are sampled, not continuous maxima.','Job lifetime peak commit is a separate metric and is not substituted for private memory.','Import completion, panel closure, animation configuration and native interaction require separate evidence.','The observer is outside the application Job and is excluded from application resource totals.')
    }
} else {
    # Compatibility observer for an already-running application. Only a fresh Job launch
    # can prove complete descendant CPU coverage, so this mode cannot pass the idle gate.
    $petRoot=Get-Process -Id $RootProcessId -ErrorAction Stop
    if($petRoot.ProcessName -notin @('token-meow','token_meow','TokenMeow')) {throw 'Root process must be the Token Meow executable.'}
    $petRootStart=$petRoot.StartTime.ToUniversalTime();$petRootExe=$petRoot.Path
    $petHashBefore=(Get-FileHash -LiteralPath $petRootExe -Algorithm SHA256).Hash
    $petLogical=[int](Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors
    $petClock=[Diagnostics.Stopwatch]::StartNew();$petPrevious=@{};$petOwned=@{};$petOwned[$RootProcessId]=$petRootStart.Ticks
    $petRecords=[Collections.Generic.List[object]]::new();$petObserved=@{};$petErrors=[Collections.Generic.List[string]]::new()
    $petExited=$false;$petCumulativeCpu100ns=0.0;$petBaselineUtc=$null
    $petGpuAvailable=$null;$petGpuError=$null;$petGpuLastAt=-10.0;$petGpuLast=@()
    while($true) {
        $petTree=@(Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,CreationDate)
        $petById=@{};foreach($petEntry in $petTree){$petById[[int]$petEntry.ProcessId]=$petEntry}
        foreach($petId in @($petOwned.Keys)) {
            $petEntry=$petById[$petId]
            if(-not $petEntry -or -not $petEntry.CreationDate -or [Math]::Abs($petEntry.CreationDate.ToUniversalTime().Ticks-$petOwned[$petId]) -gt 10) {$petOwned.Remove($petId)}
        }
        if(-not $petOwned.ContainsKey($RootProcessId)) {$petExited=$true;break}
        $petChanged=$true
        while($petChanged) {
            $petChanged=$false
            foreach($petEntry in $petTree) {
                $petParent=[int]$petEntry.ParentProcessId;$petChildId=[int]$petEntry.ProcessId
                if(-not $petEntry.CreationDate -or -not $petOwned.ContainsKey($petParent) -or $petOwned.ContainsKey($petChildId)) {continue}
                $petChild=Get-Process -Id $petChildId -ErrorAction SilentlyContinue
                if(-not $petChild){continue}
                try {
                    $petBorn=$petChild.StartTime.ToUniversalTime().Ticks
                    if([Math]::Abs($petEntry.CreationDate.ToUniversalTime().Ticks-$petBorn) -le 10 -and $petBorn -ge $petOwned[$petParent]) {
                        $petOwned[$petChildId]=$petBorn;$petChanged=$true
                    }
                } catch {$petErrors.Add("child-identity:$petChildId")}
            }
        }
        $petPrivate=0L;$petWorking=0L;$petCpuMs=0.0;$petHandles=0L;$petCurrent=@{}
        $petPerProcess=[Collections.Generic.List[object]]::new()
        foreach($petId in @($petOwned.Keys)) {
            try {
                $petProcess=Get-Process -Id $petId -ErrorAction Stop
                $petBorn=$petProcess.StartTime.ToUniversalTime()
                if($petBorn.Ticks -ne $petOwned[$petId]) {continue}
                $petIdentity="$petId|$($petBorn.Ticks)";$petTotalCpu=$petProcess.TotalProcessorTime.TotalMilliseconds
                $petProcessPrivate=$petProcess.PrivateMemorySize64;$petProcessWorking=$petProcess.WorkingSet64;$petProcessHandles=$petProcess.HandleCount
                $petAfter=Get-Process -Id $petId -ErrorAction SilentlyContinue
                if(-not $petAfter -or $petAfter.StartTime.ToUniversalTime().Ticks -ne $petBorn.Ticks){continue}
                $petCurrent[$petIdentity]=$petTotalCpu
                if($petPrevious.ContainsKey($petIdentity)) {$petCpuMs += [Math]::Max(0,$petTotalCpu-$petPrevious[$petIdentity])}
                elseif($petRecords.Count -gt 0 -and $petBorn -ge $petBaselineUtc) {$petCpuMs += $petTotalCpu}
                elseif($petRecords.Count -gt 0) {$petErrors.Add("late-preexisting-process:$petIdentity")}
                $petPrivate+=$petProcessPrivate;$petWorking+=$petProcessWorking;$petHandles+=$petProcessHandles
                $petPerProcess.Add([pscustomobject]@{id=$petId;name=$petProcess.ProcessName;startUtc=$petBorn.ToString('o');privateBytes=$petProcessPrivate;workingSetBytes=$petProcessWorking;handles=$petProcessHandles})
                $petObserved[$petIdentity]=[pscustomobject]@{id=$petId;name=$petProcess.ProcessName;startUtc=$petBorn.ToString('o')}
            } catch {
                if($petId -eq $RootProcessId){$petExited=$true}
                else { $petCheck=Get-Process -Id $petId -ErrorAction SilentlyContinue;if($petCheck){$petErrors.Add("process-counter:$petId")} }
            }
        }
        if($petExited){break}
        $petNow=$petClock.Elapsed.TotalSeconds
        if($null -eq $petBaselineUtc){$petBaselineUtc=[DateTime]::UtcNow}
        $petCumulativeCpu100ns+=$petCpuMs*10000
        # GPU rows are optional observations tied to this sampled PID/start identity.
        if($petNow-$petGpuLastAt -ge 5 -and $petGpuAvailable -ne $false) {
            try {
                $petGpuLast=@(Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine -ErrorAction Stop | Where-Object {$_.Name -match '^pid_(\d+)_' -and $petOwned.ContainsKey([int]$Matches[1])} | ForEach-Object {
                    $null=$_.Name -match '^pid_(\d+)_';$petGpuId=[int]$Matches[1]
                    [pscustomobject]@{name=$_.Name;utilizationPercentage=$_.UtilizationPercentage;processId=$petGpuId;creationTicks=$petOwned[$petGpuId]}
                });$petGpuAvailable=$true
            } catch {$petGpuAvailable=$false;$petGpuError=$_.Exception.Message;$petGpuLast=@()}
            $petGpuLastAt=$petNow
        }
        $petGpuCurrent=@($petGpuLast | Where-Object {$petOwned.ContainsKey($_.processId) -and $petOwned[$_.processId] -eq $_.creationTicks})
        $petSample=[pscustomobject]@{utc=[DateTime]::UtcNow.ToString('o');elapsedSeconds=$petNow
            privateBytes=$petPrivate;workingSetBytes=$petWorking;privateMiB=$petPrivate/1MB;workingSetMiB=$petWorking/1MB
            totalUser100ns=$petCumulativeCpu100ns;totalKernel100ns=0;cpuMilliseconds=$petCpuMs
            handles=$petHandles;processCount=$petPerProcess.Count;processes=$petPerProcess.ToArray();gpuEngines=$petGpuCurrent;gpuSampleElapsedSeconds=$petGpuLastAt}
        $petSample | ConvertTo-Json -Compress -Depth 6 | Add-Content -LiteralPath $petSamplePath -Encoding UTF8
        $petRecords.Add($petSample);$petPrevious=$petCurrent
        if($petNow-$petRecords[0].elapsedSeconds -ge $DurationSeconds){break}
        $petRemaining=$DurationSeconds-($petNow-$petRecords[0].elapsedSeconds)
        Start-Sleep -Milliseconds ([int][Math]::Max(1,[Math]::Min(1000,[Math]::Ceiling(1000*$petRemaining))))
    }
    $petHashAfter=(Get-FileHash -LiteralPath $petRootExe -Algorithm SHA256).Hash
    if($petHashBefore -ne $petHashAfter){$petErrors.Add('executable-changed-during-measurement')}
    $petMetrics=Get-MeowReducedMetrics -Samples $petRecords.ToArray() -LogicalProcessors $petLogical
    $petGate=Get-MeowResourceGate -Scenario $Scenario -RequestedSeconds $DurationSeconds -ElapsedSeconds $petMetrics.elapsed `
        -AverageCpu $petMetrics.cpu -AveragePrivate $petMetrics.averagePrivate -PeakPrivate $petMetrics.peakPrivate `
        -Completed (-not $petExited -and $petMetrics.elapsed -ge $DurationSeconds) -RootExited $petExited -Errors $petErrors.ToArray() -LogicalProcessors $petLogical
    $petSummary=[ordered]@{scenario=$Scenario;measurementMode='pid-polling';rootProcessId=$RootProcessId;rootExecutable=$petRootExe
        executableSha256=$petHashBefore;processStartUtc=$petRootStart.ToString('o');sampleEndUtc=[DateTime]::UtcNow.ToString('o')
        logicalProcessors=$petLogical;requestedDurationSeconds=$DurationSeconds;observedDurationSeconds=$petMetrics.elapsed;samples=$petRecords.Count
        rootExited=$petExited;cpuAveragePercent=$petMetrics.cpu;privateAverageMiB=$petMetrics.averagePrivate;privatePeakMiB=$petMetrics.peakPrivate
        observedProcesses=@($petObserved.Values);measurementErrors=$petErrors.ToArray();processCoverage='discoverable-descendants-only';cpuIncludesExitedProcesses=$false
        gpuCountersAvailable=$petGpuAvailable;gpuCounterError=$petGpuError;gate=$petGate.gate;thresholdGate=$petGate.thresholdGate;samplesFile=$petSamplePath
        limitations=@('PID polling cannot prove complete CPU coverage of short-lived children; use start-resource-job.ps1 and -JobMetricsPath for the idle release gate.','Private memory and GPU are sampled observations.','Import completion, panel visibility and interaction require separate evidence.','Observer CPU is excluded; discovered application descendants are included.')}
}
$petSummary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $petSummaryPath -Encoding UTF8
$petSummary | ConvertTo-Json -Depth 8
