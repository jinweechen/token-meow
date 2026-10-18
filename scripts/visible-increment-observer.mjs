// Import ONLY from the computer-use node_repl session after SKILL.md Initialize.
// This module never launches, activates, clicks, types, invokes RPC, or uses CDP.
import fs from 'node:fs/promises';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { performance } from 'node:perf_hooks';
import { fileURLToPath } from 'node:url';

const workspace = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const outputRoot = path.join(workspace, 'output');
const digest = bytes => createHash('sha256').update(bytes).digest('hex').toUpperCase();
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function outputPath(value) {
  const resolved = path.resolve(value);
  if (!resolved.toLowerCase().startsWith((outputRoot + path.sep).toLowerCase())) throw Error('Only workspace output fixture/evidence paths are accepted.');
  let cursor = resolved;
  while (cursor.length >= outputRoot.length) {
    try { if ((await fs.lstat(cursor)).isSymbolicLink()) throw Error('Fixture/evidence symlinks are not accepted.'); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
    const parent = path.dirname(cursor); if (parent === cursor) break; cursor = parent;
  }
  return resolved;
}
async function currentWindow(sky, selected) {
  if (!selected || typeof selected.app !== 'string' || !Number.isSafeInteger(selected.id)) throw Error('Pass exactly one Window returned by sky.list_apps/list_windows.');
  const matches = (await sky.list_windows()).filter(item => item.id === selected.id && item.app === selected.app);
  if (matches.length !== 1) throw Error('The selected returned window is no longer unique/current.');
  return matches[0];
}
export function parseResponseCount(accessibility) {
  const text = `${accessibility?.document_text ?? ''}\n${accessibility?.tree ?? ''}`;
  const found = [...text.matchAll(/([0-9][0-9,]*)\s*条已记录响应/g)].map(match => Number(match[1].replaceAll(',', '')));
  const unique = [...new Set(found)];
  return unique.length === 1 && Number.isSafeInteger(unique[0]) ? unique[0] : null;
}
export async function probeVisibleCounter({ sky, window }) {
  const selected = await currentWindow(sky, window);
  const state = await sky.get_window_state({ window: selected, include_screenshot: false, include_text: true });
  return { qualification: 'computer-use-read-only-counter-probe', window: state.window, responseCount: parseResponseCount(state.accessibility), accessibility: state.accessibility, observedUtc: new Date().toISOString() };
}
export async function createVisibleIncrementRun({ sky, window, fixtureManifest, processId, processCreationUtc, executablePath, expectedSha256, output }) {
  if (!Number.isInteger(processId) || processId <= 0 || !Number.isFinite(Date.parse(processCreationUtc)) || !/^[a-f0-9]{64}$/i.test(expectedSha256)) throw Error('Exact owned PID, process creation UTC and executable SHA-256 are required.');
  const manifestPath = await outputPath(fixtureManifest);
  const manifestBytes = await fs.readFile(manifestPath); const fixture = JSON.parse(manifestBytes);
  if (fixture.schemaVersion !== 1 || fixture.fixtureKind !== 'TokenMeow-visible-increment' || fixture.eventCount !== 20) throw Error('Unsupported fixture.');
  const root = path.dirname(manifestPath);
  const log = await outputPath(fixture.logPath); const data = await outputPath(fixture.dataDirectory);
  if (path.resolve(fixture.sourceRoot) !== path.join(root, 'source') || data !== path.join(root, 'data') || log !== path.join(root, 'source', 'sessions', 'token-meow-p95.jsonl')) throw Error('Fixture layout differs from its prepared layout.');
  if (digest(await fs.readFile(log)) !== fixture.initialLogSha256) throw Error('Fixture was changed/already measured; prepare a fresh fixture.');
  if (digest(await fs.readFile(executablePath)) !== expectedSha256.toUpperCase()) throw Error('Executable digest mismatch.');
  const metricsPath = path.join(data, 'desktop-metrics.json');
  const readMetrics = async () => {
    const info = await fs.stat(metricsPath);
    if (Date.now() - info.mtimeMs < -1000 || Date.now() - info.mtimeMs > 40000) throw Error('Persisted metrics file is not fresh.');
    return JSON.parse(await fs.readFile(metricsPath, 'utf8'));
  };
  const metrics = await readMetrics();
  const recent = stamp => Number.isFinite(Date.parse(stamp)) && Date.now() - Date.parse(stamp) >= -1000 && Date.now() - Date.parse(stamp) <= 40000;
  const live = value => value.processId === processId && Number.isFinite(Date.parse(value.createdAt)) && Date.parse(value.createdAt) >= Date.parse(processCreationUtc) && Date.parse(value.createdAt) <= Date.now() + 1000 && recent(value.collector?.lastSuccessAt) && value.windows?.main?.visible === true && value.windows?.main?.minimized === false && !value.shutdown;
  if (!live(metrics) || metrics.collector?.lastReport?.pendingFiles !== 0 || metrics.collector?.lastReport?.files !== 1) throw Error('Fresh owned PID, visible main window and fully collected fixture are required.');
  let selected = await currentWindow(sky, window);
  let probe = await probeVisibleCounter({ sky, window: selected }); selected = probe.window;
  if (probe.responseCount !== fixture.baselineResponses) throw Error('The selected actual Overview does not show the one prepared response.');
  const target = await outputPath(output); await fs.mkdir(path.dirname(target), { recursive: true });
  await fs.writeFile(target, '', { flag: 'wx' });
  const write = row => fs.appendFile(target, JSON.stringify(row) + '\n');
  await write({ kind: 'start', qualification: 'real-native-accessibility-with-per-increment-screenshot-review', observerSha256: digest(await fs.readFile(fileURLToPath(import.meta.url))), fixtureSha256: digest(manifestBytes), processId, processCreationUtc, selectedWindow: selected, executablePath, expectedSha256, eventsRequired: 20, thresholdMs: 5000, origin: 'before complete line write; conservative upper bound includes write/flush, accessibility and returned native screenshot observation', screenshotReviewRequired: true, utc: new Date().toISOString() });
  const clock = performance.now(); const results = []; const errors = []; let summary; let busy = false;
  async function finish() {
    if (summary) return summary;
    const values = results.filter(row => row.visible).map(row => row.latencyUpperMs).sort((a,b) => a-b);
    const p95 = values.length === 20 ? values[Math.ceil(0.95 * values.length) - 1] : null;
    const unchanged = digest(await fs.readFile(executablePath)) === expectedSha256.toUpperCase();
    summary = { kind: 'end', gate: errors.length || values.length !== 20 || !unchanged ? 'not-qualified' : p95 <= 5000 ? 'threshold-met-awaiting-independent-process-and-screenshot-review' : 'failed', qualificationScope: 'native-returned-screenshot-upper-bound-requiring-visual-review', independentProcessIdentityRequired: true, screenshotsRequireIndependentVisualReview: true, eventsObserved: values.length, eventsRequired: 20, p95UpperMs: p95, thresholdMs: 5000, errors, executableUnchanged: unchanged, fixtureLogSha256After: digest(await fs.readFile(log)), exactFirstPixelPaintTimingClaimed: false, utc: new Date().toISOString() };
    await write(summary); return summary;
  }
  async function step() {
    if (summary || results.length >= 20) return finish();
    const index = results.length + 1;
    try {
      const fresh = await readMetrics();
      if (!live(fresh)) throw Error('The selected fixture process/window is no longer active.');
      const responseId = `p95-${fixture.id}-${index}`;
      const record = { timestamp: new Date().toISOString(), type: 'token_usage_record', payload: { thread_id: fixture.threadId, response_id: responseId, turn_id: 'turn-root', root_turn_id: 'turn-root', usage: { input_tokens: 900, output_tokens: 100, cached_input_tokens: 0, cache_write_input_tokens: 0, reasoning_output_tokens: 0, total_tokens: 1000 } } };
      const before = performance.now() - clock; const writeStartedUtcMs = Date.now();
      const previousIterations = fresh.collector.iterations;
      const file = await fs.open(log, 'a'); try { await file.writeFile(JSON.stringify(record) + '\n'); await file.sync(); } finally { await file.close(); }
      const complete = performance.now() - clock; let observed; let count; let collection; let screenshot;
      do {
        if (!live(await readMetrics())) throw Error('The actual Overview must remain visible during measurement.');
        const state = await sky.get_window_state({ window: selected, include_screenshot: false, include_text: true }); selected = state.window;
        count = parseResponseCount(state.accessibility); observed = performance.now() - clock;
        if (count === fixture.baselineResponses + index) {
          const collected = await readMetrics();
          if (!live(collected) || !(collected.collector.iterations > previousIterations) || Date.parse(collected.collector.lastSuccessAt) < writeStartedUtcMs) throw Error('The current collector did not advance for this visible increment.');
          // Sky displays this actual bounded screenshot in its tool response. The
          // caller must visually verify its exact count; no script OCR, capture
          // decoding, screenshot storage or pixel timing inference is performed.
          const visual = await sky.get_window_state({ window: selected, include_screenshot: true, include_text: true });
          selected = visual.window;
          if (parseResponseCount(visual.accessibility) !== fixture.baselineResponses + index || !visual.screenshots?.length) throw Error('The screenshot observation has no matching actual statistic.');
          observed = performance.now() - clock;
          collection = { processId: collected.processId, createdAt: collected.createdAt, collector: collected.collector, mainWindow: collected.windows.main };
          screenshot = { window: selected, observedUtc: new Date().toISOString(), images: visual.screenshots.map(({id,width,height,originX,originY}) => ({id,width,height,originX,originY})), visualReviewRequired: true };
          break;
        }
        if (count > fixture.baselineResponses + index) throw Error('Unexpected extra facts appeared in this isolated fixture.');
        await delay(100);
      } while (observed - complete < 10000);
      const visible = count === fixture.baselineResponses + index;
      const row = { kind: 'event', index, responseId, expectedResponses: fixture.baselineResponses + index, observedResponses: count, writeStartedUtc: new Date(writeStartedUtcMs).toISOString(), writeStartedMs: before, completeLineFlushedMs: complete, observedMs: observed, latencyUpperMs: observed - before, completeLineFlushToObservationUpperMs: Math.max(0, observed - complete), visible, collectorBefore: { iterations: fresh.collector.iterations, lastSuccessAt: fresh.collector.lastSuccessAt }, collection, screenshot };
      results.push(row); await write(row);
      if (!visible) throw Error('Production visible statistic did not advance within ten seconds.');
      await delay(250);
      return row;
    } catch (error) {
      // A user stop ends this turn immediately; no more Computer Use/evidence actions.
      if (/user.*stop|turn.*end|interrupted|escape/i.test(String(error))) throw error;
      errors.push(String(error)); return finish();
    }
  }
  return {
    next: async () => {
      if (busy) throw Error('Observe exactly one event per serial node_repl call.');
      busy = true; try { return await step(); } finally { busy = false; }
    },
    finish: async () => {
      if (busy) throw Error('Wait for the current observation before finalizing.');
      return finish();
    },
  };
}

// Screenshot-first alternative. The original UIA-first method above and all of
// its receipts remain unchanged. Only a human can confirm the displayed count.
export async function createScreenshotIncrementRun({ sky, window, fixtureManifest, processId, processCreationUtc, executablePath, expectedSha256, output }) {
  if (!Number.isInteger(processId) || processId <= 0 || !Number.isFinite(Date.parse(processCreationUtc)) || !/^[a-f0-9]{64}$/i.test(expectedSha256)) throw Error('Exact owned PID, process creation UTC and executable SHA-256 are required.');
  const manifestPath = await outputPath(fixtureManifest);
  const manifestBytes = await fs.readFile(manifestPath); const fixture = JSON.parse(manifestBytes);
  if (fixture.schemaVersion !== 1 || fixture.fixtureKind !== 'TokenMeow-visible-increment' || fixture.eventCount !== 20) throw Error('Unsupported fixture.');
  const root = path.dirname(manifestPath);
  const log = await outputPath(fixture.logPath); const data = await outputPath(fixture.dataDirectory);
  if (path.resolve(fixture.sourceRoot) !== path.join(root, 'source') || data !== path.join(root, 'data') || log !== path.join(root, 'source', 'sessions', 'token-meow-p95.jsonl')) throw Error('Fixture layout differs from its prepared layout.');
  let expectedLog = await fs.readFile(log);
  if (digest(expectedLog) !== fixture.initialLogSha256) throw Error('Fixture was changed/already measured; prepare a fresh fixture.');
  if (digest(await fs.readFile(executablePath)) !== expectedSha256.toUpperCase()) throw Error('Executable digest mismatch.');
  const observerPath = fileURLToPath(import.meta.url);
  const observerSha256 = digest(await fs.readFile(observerPath));
  const metricsPath = path.join(data, 'desktop-metrics.json');
  const recent = stamp => Number.isFinite(Date.parse(stamp)) && Date.now() - Date.parse(stamp) >= -1000 && Date.now() - Date.parse(stamp) <= 40000;
  const readMetrics = async () => {
    const info = await fs.stat(metricsPath);
    if (Date.now() - info.mtimeMs < -1000 || Date.now() - info.mtimeMs > 40000) throw Error('Persisted metrics file is not fresh.');
    const value = JSON.parse(await fs.readFile(metricsPath, 'utf8'));
    if (value.processId !== processId || !Number.isFinite(Date.parse(value.createdAt)) || Date.parse(value.createdAt) < Date.parse(processCreationUtc) || Date.parse(value.createdAt) > Date.now() + 1000 || !recent(value.collector?.lastSuccessAt) || value.windows?.main?.visible !== true || value.windows?.main?.minimized !== false || value.shutdown) throw Error('Fresh bound fixture process/collector and visible main-window metrics are required.');
    return value;
  };
  const initial = await readMetrics();
  if (initial.collector?.lastReport?.pendingFiles !== 0 || initial.collector?.lastReport?.files !== 1 || initial.collector?.totalNewResponses !== fixture.baselineResponses) throw Error('The fresh fixture must be completely collected before measurement.');
  let selected = await currentWindow(sky, window);
  const target = await outputPath(output); await fs.mkdir(path.dirname(target), { recursive: true });
  await fs.writeFile(target, '', { flag: 'wx' });
  const write = row => fs.appendFile(target, JSON.stringify(row) + '\n');
  await write({ kind: 'start', method: 'screenshot-first-human-confirmed', qualification: 'native-screenshot-observation-awaiting-independent-review', observerSha256, fixtureSha256: digest(manifestBytes), processId, processCreationUtc, selectedWindow: selected, executablePath, expectedSha256, eventsRequired: 20, thresholdMs: 5000, maximumObservationMs: 10000, initialScreenshotDelayMs: 1000, baselineResponses: fixture.baselineResponses, baselineVisualReviewRequired: true, origin: 'before complete line write through returned native screenshot; human review delay is recorded separately', uiaCountIsDiagnosticOnly: true, utc: new Date().toISOString() });
  const clock = performance.now(); const results = []; const errors = [];
  let pending = null; let summary; let busy = false;
  const stopped = error => /user.*stop|turn.*end|interrupted|escape/i.test(String(error));
  async function finish() {
    if (summary) return summary;
    const values = results.map(row => row.latencyUpperMs).sort((a,b) => a-b);
    const p95 = values.length === 20 ? values[Math.ceil(0.95 * values.length) - 1] : null;
    const unchanged = digest(await fs.readFile(executablePath)) === expectedSha256.toUpperCase();
    const observerUnchanged = digest(await fs.readFile(observerPath)) === observerSha256;
    const fixtureLogSha256After = digest(await fs.readFile(log));
    const expectedFixtureLogSha256After = digest(expectedLog);
    const fixtureUnchangedExceptAuthoredRows = fixtureLogSha256After === expectedFixtureLogSha256After;
    const outcome = { kind: 'end', method: 'screenshot-first-human-confirmed', gate: errors.length || pending || results.length !== 20 || !unchanged || !observerUnchanged || !fixtureUnchangedExceptAuthoredRows ? 'not-qualified' : p95 <= 5000 ? 'threshold-met-awaiting-independent-process-and-visual-review' : 'failed', qualificationScope: 'twenty-human-confirmed-native-screenshot-return-upper-bounds-only', observerSha256, observerUnchanged, independentProcessIdentityRequired: true, independentVisualReviewRequired: true, eventsConfirmed: results.length, eventsRequired: 20, p95UpperMs: p95, thresholdMs: 5000, errors: [...errors], pendingIndex: pending?.index ?? null, executableUnchanged: unchanged, fixtureUnchangedExceptAuthoredRows, fixtureLogSha256After, expectedFixtureLogSha256After, endPersisted: true, exactFirstPixelPaintTimingClaimed: false, utc: new Date().toISOString() };
    try {
      await write(outcome); summary = outcome;
    } catch (error) {
      errors.push(`End evidence could not be persisted: ${String(error)}`);
      summary = { ...outcome, gate: 'not-qualified', endPersisted: false, errors: [...errors] };
    }
    return summary;
  }
  async function observeOnce() {
    if (!pending) throw Error('Append one event with next() before observing.');
    if (performance.now() - clock - pending.writeStartedMs > 10000) {
      errors.push(`Event ${pending.index}: the ten-second native observation bound expired.`);
      return finish();
    }
    const before = await readMetrics();
    selected = await currentWindow(sky, selected);
    const state = await sky.get_window_state({ window: selected, include_screenshot: true, include_text: true });
    const returnedMs = performance.now() - clock; const returnedUtc = new Date().toISOString();
    selected = state.window;
    // Never read, decode, store, OCR, crop, or re-emit screenshot pixels here.
    const images = (state.screenshots ?? []).map(({id,width,height,originX,originY}) => ({id,width,height,originX,originY}));
    const after = await readMetrics();
    const advanced = after.collector?.iterations > pending.collectorBefore.iterations && Date.parse(after.collector?.lastSuccessAt) >= pending.writeStartedUtcMs;
    const observation = { kind: 'pending-observation', method: 'screenshot-first-human-confirmed', index: pending.index, attempt: pending.observations.length + 1, responseId: pending.responseId, expectedResponses: pending.expectedResponses, writeStartedUtc: new Date(pending.writeStartedUtcMs).toISOString(), writeStartedMs: pending.writeStartedMs, completeLineFlushedMs: pending.completeLineFlushedMs, screenshotReturnedMs: returnedMs, screenshotReturnedUtc: returnedUtc, latencyUpperMs: returnedMs - pending.writeStartedMs, completeLineFlushToObservationUpperMs: Math.max(0, returnedMs - pending.completeLineFlushedMs), screenshot: { window: selected, images }, diagnosticUiaResponseCount: parseResponseCount(state.accessibility), collectorBefore: pending.collectorBefore, collectorFacts: { processId: after.processId, createdAt: after.createdAt, collector: after.collector, mainWindow: after.windows.main }, collectorAdvanced: advanced, screenshotPresent: images.length > 0, withinObservationBound: returnedMs - pending.writeStartedMs <= 10000, humanConfirmationRequired: true, nativeVisibilityAndProcessBindingRemainIndependent: true };
    pending.observations.push(observation);
    await write(observation);
    if (!images.length) errors.push(`Event ${pending.index}: native screenshot was not returned.`);
    if (!observation.withinObservationBound) errors.push(`Event ${pending.index}: native screenshot returned after ten seconds.`);
    // 'before' is intentionally read for freshness/process validation, never as a
    // UIA gate. Text may lag the visible screenshot and cannot reject it.
    void before;
    return observation;
  }
  async function serial(action) {
    if (busy) throw Error('Use one serial observation or confirmation per node_repl call.');
    busy = true;
    try { return await action(); }
    catch (error) {
      if (stopped(error)) throw error; // No further evidence/Computer Use action.
      errors.push(String(error)); return finish();
    } finally { busy = false; }
  }
  return {
    next: () => serial(async () => {
      if (summary || results.length >= 20) return finish();
      if (pending) throw Error('Confirm the current displayed screenshot before appending another event.');
      const fresh = await readMetrics();
      if (digest(await fs.readFile(log)) !== digest(expectedLog)) throw Error('Unexpected fixture mutation; this run cannot be reused.');
      const index = results.length + 1; const responseId = `p95-${fixture.id}-${index}`;
      const record = { timestamp: new Date().toISOString(), type: 'token_usage_record', payload: { thread_id: fixture.threadId, response_id: responseId, turn_id: 'turn-root', root_turn_id: 'turn-root', usage: { input_tokens: 900, output_tokens: 100, cached_input_tokens: 0, cache_write_input_tokens: 0, reasoning_output_tokens: 0, total_tokens: 1000 } } };
      const line = Buffer.from(JSON.stringify(record) + '\n');
      pending = { index, responseId, expectedResponses: fixture.baselineResponses + index, writeStartedMs: performance.now() - clock, writeStartedUtcMs: Date.now(), collectorBefore: { iterations: fresh.collector.iterations, lastSuccessAt: fresh.collector.lastSuccessAt }, observations: [] };
      const file = await fs.open(log, 'a'); try { await file.writeFile(line); await file.sync(); } finally { await file.close(); }
      expectedLog = Buffer.concat([expectedLog, line]); pending.completeLineFlushedMs = performance.now() - clock;
      await write({ kind: 'append', index, responseId, expectedResponses: pending.expectedResponses, writeStartedUtc: new Date(pending.writeStartedUtcMs).toISOString(), writeStartedMs: pending.writeStartedMs, completeLineFlushedMs: pending.completeLineFlushedMs });
      await delay(1000);
      return observeOnce();
    }),
    observeAgain: () => serial(async () => {
      if (summary) return summary;
      return observeOnce();
    }),
    confirm: ({ observedResponses } = {}) => serial(async () => {
      if (summary) return summary;
      if (!pending?.observations.length) throw Error('A returned native screenshot is required before human confirmation.');
      if (!Number.isSafeInteger(observedResponses) || observedResponses < 0) throw Error('Enter the exact response count independently read from the actual screenshot.');
      const last = pending.observations.at(-1);
      const matched = observedResponses === pending.expectedResponses;
      const accepted = matched && last.screenshotPresent && last.withinObservationBound && last.collectorAdvanced && errors.length === 0;
      const review = { kind: 'visual-review', index: pending.index, attempt: last.attempt, observedResponses, expectedResponses: pending.expectedResponses, matched, accepted, screenshotReturnedUtc: last.screenshotReturnedUtc, reviewedUtc: new Date().toISOString(), humanReviewDelayMs: Math.max(0, performance.now() - clock - last.screenshotReturnedMs), latencyUpperMs: last.latencyUpperMs, collectorAdvanced: last.collectorAdvanced };
      await write(review);
      if (!accepted) return { ...review, canObserveAgain: errors.length === 0 && performance.now() - clock - pending.writeStartedMs <= 10000, independentVisualReviewRequired: true };
      const event = { kind: 'event', index: pending.index, responseId: pending.responseId, expectedResponses: pending.expectedResponses, observedResponses, visualConfirmed: true, acceptedAttempt: last.attempt, latencyUpperMs: last.latencyUpperMs, writeStartedUtc: last.writeStartedUtc, screenshotReturnedUtc: last.screenshotReturnedUtc, collectorFacts: last.collectorFacts, screenshot: last.screenshot, observationAttempts: pending.observations.length };
      results.push(event); pending = null; await write(event);
      return { ...event, eventsConfirmed: results.length, independentProcessAndVisualReviewStillRequired: true };
    }),
    finish: () => serial(finish),
  };
}
