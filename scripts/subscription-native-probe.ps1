# Uses the documented Playwright CLI attachment, never a private browser API.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$AttemptRoot,
    [Parameter(Mandatory)][ValidateSet('Read','WatchPet','CheckPositive','CheckExpired','Quit')][string]$Stage,
    [string]$Session='subscription-native-audit',
    [string]$PlanId='',
    [string]$PositiveSubmittedAt='',
    [switch]$AppendExpiredAfterPositive
)
$ErrorActionPreference='Stop'
$subscriptionWorkspace=Split-Path -Parent $PSScriptRoot
$subscriptionRoot=(Resolve-Path -LiteralPath $AttemptRoot).Path
$subscriptionOutput=[IO.Path]::GetFullPath((Join-Path $subscriptionWorkspace 'output/validation')).TrimEnd('\')
if(-not $subscriptionRoot.StartsWith($subscriptionOutput+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected attempt root'}
$subscriptionPlan=Get-Content -LiteralPath (Join-Path $subscriptionRoot 'bootstrap.json') -Raw|ConvertFrom-Json
$subscriptionLaunch=Get-Content -LiteralPath (Join-Path $subscriptionRoot 'launch.json') -Raw|ConvertFrom-Json
if($subscriptionPlan.attemptId -ne $subscriptionLaunch.attemptId){throw 'Attempt binding differs'}
if($Stage -in @('CheckPositive','CheckExpired') -and -not $PlanId){throw 'The exact plan ID returned by this attempt is required'}
if($Stage -eq 'CheckExpired' -and -not $PositiveSubmittedAt){throw 'Use the exact previous COM lastAttemptAt; do not infer it'}
if($AppendExpiredAfterPositive -and $Stage -ne 'CheckPositive'){throw 'Automatic fixture append is only permitted immediately after the positive COM observation'}
$subscriptionArtifact=Join-Path $subscriptionWorkspace ('output/playwright/subscription-native-'+$subscriptionPlan.attemptId)
[IO.Directory]::CreateDirectory($subscriptionArtifact)|Out-Null
$subscriptionRevision=[Guid]::NewGuid().ToString('N')
$subscriptionFile=Join-Path $subscriptionArtifact ($Stage+'-'+$subscriptionRevision+'.js')
$subscriptionFactExpiry=$null
if($Stage -eq 'CheckExpired'){
    $subscriptionLate=@(Get-ChildItem -LiteralPath $subscriptionRoot -File -Filter 'native-audit-*.json'|ForEach-Object{Get-Content -LiteralPath $_.FullName -Raw|ConvertFrom-Json}|Where-Object{$_.attemptId -eq $subscriptionPlan.attemptId -and $_.tokens -eq 900 -and $_.ageSeconds -eq 10}|Sort-Object appendedAt -Descending)
    if($subscriptionLate.Count -ne 1){throw 'Exactly one authored delayed 900-Token append is required'}
    if(([DateTimeOffset]$subscriptionLate[0].appendedAt).UtcDateTime -lt ([DateTimeOffset]$PositiveSubmittedAt).UtcDateTime){throw 'Delayed fixture was appended before positive submission'}
    $subscriptionFactExpiry=([DateTimeOffset]$subscriptionLate[0].eventTime).UtcDateTime.AddSeconds(15).ToString('o')
}
$subscriptionExpected=@{stage=$Stage;processId=$subscriptionLaunch.processId;dataDirectory=$subscriptionLaunch.dataDirectory;sourceRoot=$subscriptionLaunch.sourceRoot;planId=$PlanId;positiveSubmittedAt=$PositiveSubmittedAt;factExpiry=$subscriptionFactExpiry;screenshot=(Join-Path $subscriptionArtifact ($Stage+'-'+$subscriptionRevision+'.png'))}|ConvertTo-Json -Depth 5 -Compress
$subscriptionCode=@"
async (page) => {
  const expected = $subscriptionExpected;
  const norm = value => String(value).replaceAll('\\', '/').replace(/\/+$/, '').toLowerCase();
  const read = () => page.evaluate(async () => {
    if (!window.__TAURI_INTERNALS__?.invoke) throw Error('Not a real Native Tauri page');
    const call = window.__TAURI_INTERNALS__.invoke;
    const [metrics, diagnostics, overview, snapshot] = await Promise.all([
      call('desktop_metrics'), call('desktop_diagnostics'), call('read_subscription_overview'),
      call('get_snapshot', { query: {}, includeTools: false }),
    ]);
    return { metrics, diagnostics, overview, inbox: snapshot.inbox, sources: snapshot.sources };
  });
  const assert = (value, message) => { if (!value) throw Error(message); };
  const verify = value => {
    assert(value.metrics.processId === expected.processId, 'Wrong owned application PID');
    assert(norm(value.diagnostics.dataDirectory) === norm(expected.dataDirectory), 'Wrong private data directory');
    const enabled = value.sources.filter(source => source.enabled);
    assert(enabled.length === 1 && norm(enabled[0].root) === norm(expected.sourceRoot), 'Wrong or additional enabled source');
    return value;
  };
  const host = value => {
    assert(value.metrics.runtime?.notificationStateKnown === true && value.metrics.runtime.toastSuppressed === false && value.metrics.runtime.paused === false,
      'Real Windows host is unavailable, paused or unknown; do not simulate it');
    assert(value.diagnostics.settings?.effectiveDoNotDisturb !== true && value.diagnostics.settings?.doNotDisturb !== true, 'DND must not be used to manufacture expiration rejection');
  };
  const first = verify(await read());
  if (expected.stage === 'Quit') {
    await page.evaluate(() => window.__TAURI_INTERNALS__.invoke('quit_app'));
    return { status: 'own-quit-requested', processId: expected.processId };
  }
  if (expected.stage === 'Read') return { scope: 'Actual IPC read, not Native qualification', ...first };
  assert(first.overview.capabilities.nativeNotifications === true, 'Dedicated debug audit admission is false; do not open a product flag');
  host(first);
  if (expected.stage === 'WatchPet') {
    assert(new URL(page.url()).searchParams.get('window') === 'pet', 'Select the owned pet tab using CLI tab-list/tab-select');
    const animationEnvironment = await page.evaluate(() => ({
      hidden: document.hidden,
      systemReducedMotion: matchMedia('(prefers-reduced-motion: reduce)').matches,
    }));
    assert(!animationEnvironment.hidden && !animationEnvironment.systemReducedMotion
      && first.diagnostics.settings?.reducedMotion !== true,
      'Actual pet animation is unavailable in this visible/reduced-motion state: '
        + JSON.stringify(animationEnvironment) + '; preserve the preference and record this qualification gap');
    const end = Date.now() + 20000;
    while (Date.now() < end) {
      const value = await page.evaluate(() => ({ text: document.querySelector('.pet-status-badge')?.textContent,
        clip: document.querySelector('.pet-sprite')?.getAttribute('data-animation-clip'),
        pose: document.querySelector('.pet-sprite')?.getAttribute('data-pose') }));
      if (value.text?.includes('使用计划达到提醒目标')) {
        await page.screenshot({ path: expected.screenshot });
        return { status: 'actual-pet-plan-text-observed', value, screenshot: expected.screenshot, fullG3Qualified: false };
      }
      await page.waitForTimeout(100);
    }
    throw Error('No actual pet plan text observed; preserve the failed attempt');
  }
  const end = Date.now() + 20000;
  let last = first;
  while (Date.now() < end) {
    last = verify(await read());
    host(last);
    const events = last.inbox.filter(event => event.kind === 'subscription_plan' && event.planId === expected.planId);
    if (expected.stage === 'CheckPositive' && events.length && events.every(event => event.notificationState === 'sent')) {
      assert(last.metrics.notifications?.status === 'submitted', 'Inbox sent lacks actual Windows COM submission telemetry');
      return { status: 'actual-ipc-and-com-submitted', events, notifications: last.metrics.notifications,
        runtime: last.metrics.runtime, shownOnScreenProven: false, fullG3Qualified: false };
    }
    if (expected.stage === 'CheckExpired' && events.length && events.every(event => event.notificationState === 'suppressed')) {
      assert(last.metrics.notifications?.lastAttemptAt === expected.positiveSubmittedAt, 'A second COM attempt happened; expired queue rejection is not proven');
      assert(events.every(event => event.notifyEligible === true), 'These were panel-only events, not a queued expiring Native attempt');
      assert(Date.now() >= Date.parse(expected.factExpiry), 'Suppression happened before the authored fact deadline; expiration is not proven');
      const plan = last.overview.plans.find(plan => plan.id === expected.planId);
      assert(plan?.lifecycle === 'active' && plan.remindersEnabled === true && plan.target === 1000 && plan.used === 1000,
        'A changed/ended/paused/unknown plan cannot establish expiration rejection');
      return { status: 'actual-expired-queue-suppressed-without-new-com-attempt', events,
        notifications: last.metrics.notifications, fullG3Qualified: false };
    }
    if (events.some(event => event.notificationState === 'failed')) throw Error('Windows notification submission failed: '+JSON.stringify(events));
    await page.waitForTimeout(250);
  }
  throw Error('Native stage timed out; latest state='+JSON.stringify({inbox:last.inbox,notifications:last.metrics.notifications,runtime:last.metrics.runtime}));
}
"@
[IO.File]::WriteAllText($subscriptionFile,$subscriptionCode,[Text.UTF8Encoding]::new($false))
Push-Location $subscriptionWorkspace
try {
    & npx --offline --yes --package '@playwright/cli' playwright-cli "-s=$Session" run-code "--filename=$subscriptionFile" 2>&1 | Tee-Object -FilePath (Join-Path $subscriptionArtifact ($Stage+'-'+$subscriptionRevision+'.log'))
    $subscriptionExit=$LASTEXITCODE
}finally{Pop-Location}
if($subscriptionExit -eq 0 -and $AppendExpiredAfterPositive){
    & (Join-Path $PSScriptRoot 'subscription-native-append.ps1') -AttemptRoot $subscriptionRoot -Tokens 900 -AgeSeconds 10
}
exit $subscriptionExit
