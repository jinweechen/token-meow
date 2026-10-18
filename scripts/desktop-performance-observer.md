# Desktop performance evidence

These helpers do not change the application, invoke product commands, enable a
debug port, or launch the application. The native acceptance owner runs the final
release and handles its UI. Existing `start-resource-job.ps1` and
`measure-resources.ps1` retain the frozen 600-second/1%/350 MiB gates.

## Readiness and idle prerequisites

Use an isolated source/data directory. Build that ledger once, select reduced
motion in the application, and exit normally. Reuse the committed database for
the warm-start measurement. Record a native observation that the pet has consumed
the static preference; a stored setting alone does not prove rendered behavior.

Before creating the resource Job, start the following observer in a separate
tool session. Choose fresh output paths. It waits at most 30 seconds for the new
Job start, rejects old PID/creation-time metrics, and never waits for an account
network request to accept the actual frontend-ready signal.

```powershell
python scripts/observe-runtime-preconditions.py `
  --data '<isolated own data>' --source-root '<isolated source>' `
  --job '<new output/performance/idle-job.jsonl>' `
  --out '<new output/performance/runtime-state.jsonl>' --ready-window pet
```

Then the acceptance owner launches the exact release with
`start-resource-job.ps1 -ExpectedSha256 ... -ArgumentList @('--background')
-WarmupSeconds 60 -DurationSeconds 600 -SampleMilliseconds 1000`. Startup metadata
collection should happen before creating its suspended process, to avoid charging
observer bookkeeping to the conservative readiness upper bound. The observer
must have started before process creation and the ledger must already exist for
warm qualification. For an ordinary main-panel warm-start scenario use
`--ready-window main` and launch without `--background` in a separate run.

Readiness is measured from the real Job root creation FILETIME to the first
external observation of that PID's persisted ready signal. This upper bound
includes observer/persistence latency and suspension time, and adds twice the
observed maximum clock skew plus 1 ms for timestamp rounding. The application's
`frontendReadyMs` is kept separately; it begins at `desktop::run` and is not
substituted for whole-process time. Wall/monotonic clock skew over 50 ms prevents
qualification. This helper does not claim keyboard, image-decode, or pixel-paint
timing beyond the application's two-RAF/bindings readiness contract.

During the resource interval, the observer reads only own-ledger settings and
whitelisted metrics. It requires exactly the selected enabled source, known zero
history work, no pause/cancel, zero pending files, a fresh collector, main hidden,
pet visible and unminimized, desktop connected/unlocked/awake with display on,
power saver off, and an explicit reducedMotion preference. It retains state failures,
requires samples spanning both Job endpoints with no gap above 2.5 seconds, and
reports `passed-runtime-preconditions-only`. Combine it with the separately reduced
Job resource result and native setup observation. Neither helper alone qualifies
the release. Errors, partial files, missing ready signals, and stale metrics remain
unqualified. Existing output files are never overwritten.

## Twenty real visible increments

1. Run `./scripts/prepare-incremental-fixture.ps1`. It creates one new directory
   under output/performance, copies only the fixed content-free JSONL fixture,
   dates it to current UTC, and returns CODEX_HOME/TOKEN_MEOW_DATA_DIR. It never
   starts an app. Initial oracle: one response, 1200 tokens.
2. The native acceptance owner starts the exact release against those directories,
   opens the main Overview, and verifies import completion. Do not filter away the
   fixture. This is a different scenario from the main-hidden idle run.
3. Read the computer-use SKILL and guidance. Initialize `@oai/sky` in node_repl,
   call `sky.list_apps()`/`list_windows()`, print the returned candidates, and select
   exactly one returned main window belonging to the verified application process.
   Do not invent a window ID or reconstruct a Window object.
4. Import `visible-increment-observer.mjs` inside that same node_repl session:

```javascript
globalThis.meowObserver = await import(
  (await import('node:url')).pathToFileURL('D:\\project\\宠物\\scripts\\visible-increment-observer.mjs').href
);
globalThis.meowProbe = await meowObserver.probeVisibleCounter({sky, window: targetWindow});
nodeRepl.write(JSON.stringify(meowProbe));
```

Stop and inspect the returned actual accessibility observation. If its one
response counter cannot be read, do not append test events or replace the result
with an RPC/SQLite timing. Report the missing observation capability first.

5. Create the run, passing the exact PID/creation UTC/executable digest obtained
   through ordinary read-only process inspection. `targetWindow` must still be the
   Window returned and inspected above.

```javascript
globalThis.meowIncrementRun = await meowObserver.createVisibleIncrementRun({
  sky, window: targetWindow, processId: /* verified PID */,
  processCreationUtc: /* verified process StartTime UTC */,
  executablePath: /* exact release EXE */, expectedSha256: /* frozen digest */,
  fixtureManifest: /* prepared fixture.json */, output: /* fresh output JSONL */
});
```

6. In serial node_repl calls, run `nodeRepl.write(JSON.stringify(await
   meowIncrementRun.next()))` exactly twenty times. Each call appends one complete
   native usage line to only the prepared fixture, then uses the supported Sky
   read-only state API to observe the real visible response count. There is no
   mouse/keyboard input, activation, CDP, or application RPC. Once the count matches,
   it obtains a real bounded native screenshot through Sky, which is displayed in
   that tool response. Independently inspect each screenshot for the expected count
   and actual visible main panel before proceeding. Do not save, decode or re-emit
   the screenshot merely for inspection. Each event retains the exact returned
   window/image metadata, before/after collector facts, write-start, flush-complete,
   and screenshot-return timestamps. The gate uses the conservative
   write-start-to-screenshot-return upper bound. The separate
   `completeLineFlushToObservationUpperMs` is also an upper bound, not a bound on
   the earliest paint. Stop on any error/end result; never invent missing samples.
7. Call `nodeRepl.write(JSON.stringify(await meowIncrementRun.finish()))`. Only
   twenty observed unique increments and P95 <=5000 ms can report
   `threshold-met-awaiting-independent-process-and-screenshot-review`. The helper
   cannot bind a Sky opaque Window to an OS PID/image or independently review its
   screenshot pixels. It never reports a qualified product pass. The acceptance
   owner separately binds the running image/PID/creation identity to the returned
   Sky app/window and reviews all twenty displayed images. Cached metrics do not
   independently prove native visibility. The helper records its own source hash,
   requires fresh metrics/collector timestamps and a collector iteration advancing
   after each appended line. A visibly correct screenshot supplies a conservative
   observation upper bound; the earliest paint time and the entire M4 gate remain
   outside the helper's automatic claim. Earlier C11 timing-only receipts are
   preserved under their original observation method.
   If Computer Use reports Escape/user stop/turn ended, stop the turn immediately;
   leave the partial evidence and do not call further methods for cleanup.

The original source fixture and existing real Codex sources are never appended to.
The prepared copy cannot be reused after an attempted measurement. Earlier failure
receipts remain intact; choose a fresh prepared fixture for a justified rerun.

## Screenshot-first, manually confirmed alternative

`createScreenshotIncrementRun` is a separate method for native cases where UIA
text lags the visible statistic. Earlier UIA-first failures and receipts stay
unchanged. Use a fresh prepared fixture and independently verify the baseline
screenshot and exact running process/window identity before creating this run.
No UIA response count is used as a positive or negative acceptance gate.

Re-import the reviewed module with a fresh URL query in a persistent node_repl
session (ES modules otherwise remain cached), then pass the same verified
arguments as above to `createScreenshotIncrementRun`. The start receipt records
the exact current helper source SHA256 and method name.

```javascript
globalThis.meowScreenshotObserver = await import(
  (await import('node:url')).pathToFileURL('D:\\project\\宠物\\scripts\\visible-increment-observer.mjs').href + '?method=screenshot-first-v1'
);
globalThis.meowScreenshotRun = await meowScreenshotObserver.createScreenshotIncrementRun({
  sky, window: targetWindow, processId: /* verified PID */,
  processCreationUtc: /* verified process StartTime UTC */,
  executablePath: /* exact release EXE */, expectedSha256: /* frozen digest */,
  fixtureManifest: /* new fixture.json */, output: /* new output JSONL */
});
nodeRepl.write(JSON.stringify(await meowScreenshotRun.next()));
```

Each `next()` appends exactly one new unique usage line, waits 1000 ms, and calls
the supported passive native screenshot API once. It returns a pending observation
with screenshot metadata and the write-start-to-screenshot-return upper bound.
Review the actual image displayed by Sky, including the visible response count.
In the next serial cell call
`nodeRepl.write(JSON.stringify(await meowScreenshotRun.confirm({observedResponses: N})))`
with the exact count you independently read. The helper cannot read the pixels.
Human review time is retained separately and is not substituted for screenshot
return time. Do not save, decode, OCR, crop, or re-emit the screenshot.

If the count is wrong, `confirm` preserves that rejected review without accepting
the event. Call `observeAgain()` within ten seconds of the original write, inspect
the new returned native image, and confirm again. Re-observation never appends an
extra line. There is no UIA polling or automatic inference of visible success.
Calling `next()` while an event is pending is invalid. A correct confirmation
also requires advancing, fresh collector facts for the bound process.

After twenty accepted unique events, call `finish()`. Nearest-rank P95 <=5000 ms
can only produce `threshold-met-awaiting-independent-process-and-visual-review`;
partial events, missing screenshots, observation bounds/errors, or changed EXE,
helper source or fixture bytes remain unqualified. The acceptance owner must
still review every native image and bind the Sky window to PID/creation/image.
Escape/user-stop/turn-ended always ends the turn immediately; leave partial
evidence and do not invoke cleanup methods. This method has no qualification
until it is actually executed and independently reviewed.
