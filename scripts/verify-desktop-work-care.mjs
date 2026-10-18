// Real embedded debug UI + agent-authored rows in a strictly isolated fixture.
// Does not change source-tool configuration, issue model calls or use global input.
import fs from 'node:fs/promises';
import path from 'node:path';
import { createHash, randomUUID } from 'node:crypto';
import { createRequire } from 'node:module';
import assert from 'node:assert/strict';

const options = Object.fromEntries(process.argv.slice(2).map(arg => {
  const index = arg.indexOf('=');
  return [arg.slice(2, index), arg.slice(index + 1)];
}));
const pid = Number(options['process-id']);
if (!Number.isSafeInteger(pid) || pid < 1 || !options.data || !options.source) throw Error('Owned fixture PID/source/data are required.');
const root = await fs.realpath('output');
const inside = value => value.toLowerCase().startsWith((root + path.sep).toLowerCase());
const data = await fs.realpath(options.data), source = await fs.realpath(options.source);
const output = path.resolve(options.out ?? 'output/native-work-care.json');
assert.ok(inside(data) && inside(source) && inside(output));
try { await fs.access(output); throw Error('Evidence already exists.'); } catch (error) { if (error.code !== 'ENOENT') throw error; }
const fixture = await fs.realpath(path.join(source, 'sessions', 'fixture.jsonl'));
assert.ok(fixture.toLowerCase().startsWith((source + path.sep).toLowerCase()));
const endpoint = new URL(options.cdp ?? 'http://127.0.0.1:9222');
assert.ok(endpoint.protocol === 'http:' && ['127.0.0.1', 'localhost', '[::1]'].includes(endpoint.hostname));
const require = createRequire(import.meta.url);
const { chromium } = require(path.join(process.env.TOKEN_MEOW_TEST_NODE_MODULES ?? '', 'playwright'));
const browser = await chromium.connectOverCDP(endpoint.toString());
const report = { observedAt: new Date().toISOString(), processId: pid,
  scope: 'real debug native Core/collector/embedded UI with agent-authored isolated JSONL and targeted CDP input',
  limitations: ['No physical desktop click, release performance, installation or real model task is asserted.',
    'Fixture changes are authored by this verifier, not by the read-only collector.',
    'Long-duration reminders and all event outcomes retain separate controller/browser evidence.'], cases: [], authoredRows: [] };
let pet;
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
try {
  const pages = browser.contexts().flatMap(context => context.pages()).filter(page => new URL(page.url()).hostname === 'tauri.localhost');
  const pets = pages.filter(page => new URL(page.url()).searchParams.get('window') === 'pet');
  assert.equal(pets.length, 1); pet = pets[0];
  const invoke = (command, args = {}) => pet.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const snapshot = () => invoke('get_snapshot', { query: {} });
  const diagnostic = await invoke('desktop_diagnostics'), metrics = await invoke('desktop_metrics');
  assert.equal(metrics.processId, pid);
  assert.equal((await fs.realpath(diagnostic.dataDirectory)).toLowerCase(), data.toLowerCase());
  const before = await snapshot();
  assert.equal(before.sources.filter(s => s.enabled).length, 1);
  assert.equal((await fs.realpath(before.sources.find(s => s.enabled).root)).toLowerCase(), source.toLowerCase());
  assert.deepEqual([before.settings.workCareEnabled, before.settings.careTone, before.settings.careIntervalMinutes], [true, 'mixed', 25]);
  assert.equal(before.overview.responseCount, 1); assert.equal(before.overview.totalTokens, 1200);
  report.fixtureSha256Before = hash(await fs.readFile(fixture));
  const care = pet.locator('.pet-care-bubble');
  const observe = async label => {
    const value = await pet.evaluate(() => {
      const node = document.querySelector('.pet-care-bubble');
      return { kind: node?.getAttribute('data-care-kind') ?? null, tone: node?.getAttribute('data-care-tone') ?? null,
        text: node?.querySelector('p')?.textContent ?? null, bounds: node?.getBoundingClientRect().toJSON() ?? null,
        badge: document.querySelector('.pet-status-badge')?.textContent,
        htmlBackground: getComputedStyle(document.documentElement).backgroundColor,
        frame: document.querySelector('.pet-sprite')?.getAttribute('data-frame') };
    });
    report.cases.push({ label, ...value }); return value;
  };
  const waitKind = async kind => { await pet.locator(`.pet-care-bubble[data-care-kind="${kind}"]`).waitFor({ state: 'visible', timeout: 12_000 }); return observe(kind); };
  await pet.keyboard.press('Escape');
  await pet.waitForTimeout(3300); // Ensure first actual frontend snapshot has primed its watermark.
  assert.equal(await care.count(), 0); await observe('cold startup history suppressed');
  await pet.getByRole('button', { name: '摸摸小柚', exact: true }).click();
  const first = await waitKind('tap');
  assert.equal(first.htmlBackground, 'rgba(0, 0, 0, 0)');
  await pet.getByRole('button', { name: '关闭关心语', exact: true }).click();
  await pet.waitForTimeout(3200);
  await pet.getByRole('button', { name: '摸摸小柚', exact: true }).click();
  const second = await waitKind('tap');
  assert.notEqual(first.tone, second.tone);
  assert.ok(second.bounds.x >= 0 && second.bounds.right <= 280 && second.bounds.bottom <= 340);
  await pet.getByRole('button', { name: '关闭关心语', exact: true }).click();
  const turn = `care-${randomUUID()}`;
  const started = new Date().toISOString();
  const append = async rows => { await fs.appendFile(fixture, rows.map(row => JSON.stringify(row)).join('\n') + '\n'); report.authoredRows.push(...rows); };
  await append([
    { timestamp: started, type: 'turn_context', payload: { turn_id: turn, root_turn_id: turn, model: 'fixture-model' } },
    { timestamp: started, type: 'event_msg', payload: { type: 'task_started', turn_id: turn, root_turn_id: turn, started_at: started } },
  ]);
  const start = await waitKind('start');
  assert.match(start.badge, /活动中/);
  const completed = new Date().toISOString();
  await append([
    { timestamp: completed, type: 'token_usage_record', payload: { thread_id: '00000000-0000-4000-8000-000000000001', turn_id: turn, root_turn_id: turn, response_id: `response-${turn}`, usage: { input_tokens: 10, output_tokens: 2, total_tokens: 12, cached_input_tokens: 0, cache_write_input_tokens: 0, reasoning_output_tokens: 0 } } },
    { timestamp: completed, type: 'event_msg', payload: { type: 'task_complete', status: 'completed', turn_id: turn, root_turn_id: turn, started_at: started, completed_at: completed } },
  ]);
  const done = await waitKind('complete');
  assert.match(done.text, /正常/);
  await pet.screenshot({ path: output.replace(/\.json$/, '-complete.png') });
  const after = await snapshot();
  assert.equal(after.overview.responseCount, 2); assert.equal(after.overview.totalTokens, 1212);
  assert.equal(after.activity.activeTurns, 0);
  report.actualOverview = after.overview;
  await pet.getByRole('button', { name: /打开主面板/, exact: false }).click();
  await care.waitFor({ state: 'detached', timeout: 3000 });
  const opened = await invoke('desktop_diagnostics');
  assert.ok(opened.windows.some(w => w.label === 'main' && w.visible && w.focused));
  report.cases.push({ label: 'care CTA opens the real native main panel', main: opened.windows.find(w => w.label === 'main') });
  await invoke('run_action', { name: 'update_settings', payload: { workCareEnabled: false } });
  await pet.waitForTimeout(3300);
  await pet.getByRole('button', { name: '摸摸小柚', exact: true }).click();
  await pet.waitForTimeout(200); assert.equal(await care.count(), 0);
  assert.ok(await pet.locator('.pet-sprite').count() === 1);
  await observe('disabled care keeps the character and suppresses speech');
  await invoke('run_action', { name: 'update_settings', payload: { workCareEnabled: true, doNotDisturb: true } });
  await pet.locator('.pet-sprite[data-frame="4"]').waitFor({ state: 'visible', timeout: 6000 });
  await pet.getByRole('button', { name: '摸摸小柚', exact: true }).click();
  await pet.waitForTimeout(200); assert.equal(await care.count(), 0);
  await observe('native DND suppresses care and uses sleeping pose');
  await invoke('run_action', { name: 'update_settings', payload: { doNotDisturb: false } });
  await pet.waitForTimeout(3300); assert.equal(await care.count(), 0);
  await observe('DND recovery does not replay old care');
  report.fixtureSha256After = hash(await fs.readFile(fixture));
  report.status = 'passed';
} catch (error) {
  report.status = 'failed'; report.error = String(error); process.exitCode = 1;
  if (pet) { try { report.failureText = (await pet.locator('body').innerText()).slice(0, 2000); } catch {} }
} finally {
  await fs.writeFile(output, JSON.stringify(report, null, 2) + '\n');
  await browser.close().catch(() => {});
}
process.stdout.write(JSON.stringify({ status: report.status, cases: report.cases.length, error: report.error, evidence: output }, null, 2) + '\n');
