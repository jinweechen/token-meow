# Pure reduction and validation shared by the live observer and fixed fixtures.
function Get-MeowResourceGate {
    param([string]$Scenario,[int]$RequestedSeconds,[double]$ElapsedSeconds,
        $AverageCpu,$AveragePrivate,$PeakPrivate,[bool]$Completed,[bool]$RootExited,
        [string[]]$Errors=@(),[bool]$Contained=$false,[bool]$Fixture=$false,[int]$LogicalProcessors=12)
    $petThreshold=if($Scenario -eq 'lifecycle') {'lifecycle-sampling-only'}
        elseif($RootExited -or -not $Completed -or $ElapsedSeconds -lt $RequestedSeconds -or $Errors.Count -gt 0 -or $null -eq $PeakPrivate -or $null -eq $AveragePrivate) {'not-qualified'}
        elseif($Scenario -eq 'import') {if($PeakPrivate -le 600){'memory-only-passed-import-completion-not-proven'}else{'failed'}}
        elseif($RequestedSeconds -lt 600 -or $ElapsedSeconds -lt 600 -or $null -eq $AverageCpu -or $LogicalProcessors -ne 12) {'not-qualified'}
        elseif($AverageCpu -le 1 -and $AveragePrivate -le 350 -and $PeakPrivate -le 350) {'passed'}else{'failed'}
    $petGate=if($Fixture){'fixture-only'}
        elseif($petThreshold -eq 'passed' -and -not $Contained){'not-qualified-process-coverage'}else{$petThreshold}
    return [pscustomobject]@{gate=$petGate;thresholdGate=$petThreshold}
}

function Get-MeowReducedMetrics {
    param([object[]]$Samples,[int]$LogicalProcessors)
    if($Samples.Count -lt 2 -or $LogicalProcessors -le 0) {
        return [pscustomobject]@{elapsed=0.0;cpu=$null;averagePrivate=$null;peakPrivate=$null;cpu100ns=$null}
    }
    $petFirst=$Samples[0];$petLast=$Samples[$Samples.Count-1]
    $petElapsed=[double]$petLast.elapsedSeconds-[double]$petFirst.elapsedSeconds
    if($petElapsed -le 0) {throw 'A positive, monotonic measurement interval is required.'}
    $petCpu=([decimal]$petLast.totalUser100ns+[decimal]$petLast.totalKernel100ns)-([decimal]$petFirst.totalUser100ns+[decimal]$petFirst.totalKernel100ns)
    $petIntegral=0.0;$petPeak=0.0
    for($petIndex=0;$petIndex -lt $Samples.Count;$petIndex++) {
        $petValue=[double]$Samples[$petIndex].privateBytes
        $petPeak=[Math]::Max($petPeak,$petValue)
        if($petIndex -gt 0) {
            $petDelta=[double]$Samples[$petIndex].elapsedSeconds-[double]$Samples[$petIndex-1].elapsedSeconds
            $petIntegral += $petDelta*($petValue+[double]$Samples[$petIndex-1].privateBytes)/2
        }
    }
    [pscustomobject]@{elapsed=$petElapsed;cpu=100.0*[double]$petCpu/(10000000.0*$petElapsed*$LogicalProcessors)
        averagePrivate=$petIntegral/$petElapsed/1MB;peakPrivate=$petPeak/1MB;cpu100ns=$petCpu}
}

function Read-MeowJobEvidence {
    param([string]$Path,[int]$RequestedSeconds)
    $petInfo=Get-Item -LiteralPath $Path -ErrorAction Stop
    if($petInfo.Length -gt 128MB) {throw 'Job evidence exceeds the 128 MiB reader bound.'}
    $petRows=[Collections.Generic.List[object]]::new()
    foreach($petLine in [IO.File]::ReadLines($petInfo.FullName)) {
        if($petLine.Length -gt 1MB -or $petRows.Count -ge 80000) {throw 'Job evidence exceeds its record bound.'}
        if(-not [string]::IsNullOrWhiteSpace($petLine)) {$petRows.Add(($petLine | ConvertFrom-Json))}
    }
    if($petRows.Count -lt 4) {throw 'Job evidence requires start, baseline, endpoint, and end records.'}
    $petStart=$petRows[0];$petEnd=$petRows[$petRows.Count-1]
    if($petStart.kind -ne 'start' -or $petStart.schemaVersion -ne 1 -or $petEnd.kind -ne 'end') {throw 'Incomplete or unsupported Job evidence.'}
    foreach($petField in @('assignBeforeResume','breakawayAllowed','helperOutsideJob','jobCpuIncludesExited','isFixture')) {
        if($petStart.$petField -isnot [bool]) {throw "Start flag must be an explicit boolean: $petField."}
    }
    foreach($petField in @('rootExited','completed')) {
        if($petEnd.$petField -isnot [bool]) {throw "End flag must be an explicit boolean: $petField."}
    }
    if([string]::IsNullOrWhiteSpace($petStart.jobId) -or $petStart.rootProcessId -le 0 -or $petStart.rootCreationFileTime -le 0) {throw 'Job/root identity is missing.'}
    if($petStart.assignBeforeResume -ne $true -or $petStart.breakawayAllowed -ne $false -or $petStart.helperOutsideJob -ne $true -or $petStart.jobCpuIncludesExited -ne $true) {throw 'Required Job containment guarantees are absent.'}
    if($petStart.executableSha256 -notmatch '^[0-9a-fA-F]{64}$' -or $petStart.executableSha256 -ne $petEnd.executableSha256After) {throw 'Executable digest is missing or changed during measurement.'}
    if($petStart.requestedSeconds -ne $RequestedSeconds) {throw 'Requested interval does not match the Job evidence.'}
    $petLogical=[int]$petStart.logicalProcessors
    if($petLogical -le 0 -or $petLogical -gt 4096) {throw 'Invalid logical processor count.'}
    if($petStart.isFixture -ne $true -and [IO.Path]::GetFileNameWithoutExtension($petStart.executablePath) -notin @('token-meow','token_meow','TokenMeow')) {throw 'Job evidence is not for the application.'}
    $petErrors=[Collections.Generic.List[string]]::new();$petSamples=[Collections.Generic.List[object]]::new()
    $petPrevious=$null
    for($petIndex=1;$petIndex -lt $petRows.Count-1;$petIndex++) {
        $petRow=$petRows[$petIndex]
        if($petRow.kind -ne 'sample' -or $petRow.jobId -ne $petStart.jobId) {throw 'Unexpected record or mixed Job identity.'}
        if($petRow.rootExited -isnot [bool]) {throw 'Each sample must explicitly report root lifetime.'}
        foreach($petField in @('elapsedSeconds','totalUser100ns','totalKernel100ns','privateBytes','workingSetBytes','handleCount','activeProcesses','totalProcesses','terminatedProcesses')) {
            if($null -eq $petRow.$petField) {throw "Sample is missing $petField."}
            $petNumber=[double]$petRow.$petField
            if([double]::IsNaN($petNumber) -or [double]::IsInfinity($petNumber) -or $petNumber -lt 0) {throw "Invalid sample field: $petField."}
        }
        if($petRow.totalProcesses -lt $petRow.activeProcesses) {throw 'Job process accounting is inconsistent.'}
        if($null -eq $petRow.PSObject.Properties['measurementErrors'] -or $null -eq $petRow.measurementErrors) {throw 'Sample coverage/errors must be explicit.'}
        foreach($petError in $petRow.measurementErrors) {$petErrors.Add([string]$petError)}
        if($petRow.rootExited) {$petErrors.Add('root-exited-during-sample')}
        if($petPrevious) {
            if($petRow.elapsedSeconds -le $petPrevious.elapsedSeconds -or $petRow.totalUser100ns -lt $petPrevious.totalUser100ns -or $petRow.totalKernel100ns -lt $petPrevious.totalKernel100ns -or $petRow.totalProcesses -lt $petPrevious.totalProcesses) {throw 'Job clock/cumulative accounting moved backwards.'}
            # A stalled sampler cannot prove the frozen one-second memory sampling method.
            if($petRow.elapsedSeconds-$petPrevious.elapsedSeconds -gt 2.5) {$petErrors.Add('sample-gap-exceeds-2.5-seconds')}
        }
        $petSamples.Add($petRow);$petPrevious=$petRow
    }
    if($petEnd.jobId -ne $petStart.jobId -or $petEnd.elapsedSeconds -ne $petPrevious.elapsedSeconds) {throw 'End record does not match the final sample.'}
    if($null -eq $petEnd.PSObject.Properties['measurementErrors'] -or $null -eq $petEnd.measurementErrors) {throw 'End coverage/errors must be explicit.'}
    foreach($petError in $petEnd.measurementErrors) {$petErrors.Add([string]$petError)}
    $petMetrics=Get-MeowReducedMetrics -Samples $petSamples.ToArray() -LogicalProcessors $petLogical
    return [pscustomobject]@{start=$petStart;end=$petEnd;samples=$petSamples.ToArray();errors=$petErrors.ToArray();metrics=$petMetrics}
}
