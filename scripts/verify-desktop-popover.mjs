// Targeted regression against the real embedded WebView and native focus events.
// It activates only this fixture application's own windows; no global input is sent.
import fs from 'node:fs/promises';
import path from 'node:path';
import { createRequire } from 'node:module';
import assert from 'node:assert/strict';

const options = Object.fromEntries(process.argv.slice(2).map(arg => {
  const separator = arg.indexOf('=');
  return [arg.slice(2, separator), arg.slice(separator + 1)];
}));
const expectedProcess = Number(options['process-id']);
if (!Number.isSafeInteger(expectedProcess) || expectedProcess < 1 || !options.data) {
  throw Error('--process-id=<owned fixture PID> and --data=<isolated data directory> are required.');
}
const endpoint = new URL(options.cdp ?? 'http://127.0.0.1:9222');
if (endpoint.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(endpoint.hostname)) throw Error('Only the owned loopback test endpoint is accepted.');
const outputRoot = await fs.realpath(path.resolve('output'));
const target = path.resolve(options.out ?? 'output/native-popover-regression.json');
if (!target.startsWith(outputRoot + path.sep) || path.extname(target) !== '.json') throw Error('Evidence must be a workspace output JSON.');
const expectedData = await fs.realpath(options.data);
if (!expectedData.startsWith(outputRoot + path.sep)) throw Error('Native regression must use isolated workspace fixture data.');
const require = createRequire(import.meta.url);
const { chromium } = require(path.join(process.env.TOKEN_MEOW_TEST_NODE_MODULES ?? '', 'playwright'));
const browser = await chromium.connectOverCDP(endpoint.toString());
const report = { observedAt: new Date().toISOString(), processId: expectedProcess,
  scope: 'real native debug WebViews and OS focus transfer between this fixture app windows',
  limitations: ['CDP mouse/keyboard events target the embedded page; they do not prove a physical desktop click or global keyboard delivery.',
    'Native blur is exercised by activating the application main window through its real open_panel command.',
    'Not a release, multi-monitor, click-through, CPU or memory qualification.'], cases: [] };
let pet, failureObservation;
try {
  const pages = browser.contexts().flatMap(context => context.pages()).filter(page => {
    const url = new URL(page.url());
    return url.hostname === 'tauri.localhost';
  });
  const pets = pages.filter(page => new URL(page.url()).searchParams.get('window') === 'pet');
  const mains = pages.filter(page => new URL(page.url()).searchParams.get('window') !== 'pet');
  assert.equal(pets.length, 1); assert.equal(mains.length, 1);
  pet = pets[0];
  const main = mains[0];
  const invoke = (page, command) => page.evaluate(command => window.__TAURI_INTERNALS__.invoke(command), command);
  const diagnostics = await invoke(pet, 'desktop_diagnostics');
  const metrics = await invoke(pet, 'desktop_metrics');
  assert.equal(metrics.processId, expectedProcess, 'CDP endpoint belongs to another process');
  assert.equal((await fs.realpath(diagnostics.dataDirectory)).toLowerCase(), expectedData.toLowerCase(), 'Refuse to operate on non-fixture application data');
  if (options['quit-only'] === 'true') {
    await invoke(pet, 'quit_app').catch(error => {
      if (!/closed|Target.*destroyed/i.test(String(error))) throw error;
    });
    report.status = 'quit-requested';
  } else if (options['pose-only'] === 'true') {
    report.scope = 'real embedded native debug WebView; production affection button and controlled second PNG sheet';
    const affection = pet.getByRole('button', { name: '摸摸小柚', exact: true });
    assert.equal(await affection.count(), 1);
    assert.equal(await pet.locator('.pet-popover').count(), 0);
    await affection.click();
    const sprite = pet.locator('.pet-sprite[data-pose="tap"][data-sheet="cute"]');
    await sprite.waitFor({ state: 'visible', timeout: 1500 });
    const pose = await sprite.evaluate(element => ({
      frame: element.dataset.frame,
      sheet: element.dataset.sheet,
      pose: element.dataset.pose,
      background: getComputedStyle(document.documentElement).backgroundColor,
      rectangle: element.getBoundingClientRect().toJSON(),
    }));
    assert.ok(Number.isSafeInteger(Number(pose.frame)) && Number(pose.frame) >= 6 && Number(pose.frame) <= 12);
    assert.equal(pose.background, 'rgba(0, 0, 0, 0)');
    assert.equal(await pet.locator('.pet-popover').count(), 0);
    const screenshot = path.join(path.dirname(target), `${path.basename(target, '.json')}.png`);
    await pet.screenshot({ path: screenshot });
    report.cases.push({ label: 'affection uses the real second PNG sheet while preserving transparency', ...pose, screenshot });
    report.status = 'passed';
  } else {
  const observe = async label => {
    const state = await invoke(pet, 'desktop_diagnostics');
    const metric = await invoke(pet, 'desktop_metrics');
    const dom = await pet.evaluate(() => ({ menu: document.querySelectorAll('.pet-menu').length,
      card: document.querySelectorAll('.pet-stats').length,
      htmlBackground: getComputedStyle(document.documentElement).backgroundColor,
      bodyBackground: getComputedStyle(document.body).backgroundColor,
      focusedElement: document.activeElement?.tagName,
    }));
    const record = { label, dom, windows: state.windows, petInteraction: metric.petInteraction ?? null };
    report.cases.push(record); return record;
  };
  failureObservation = observe;
  const settled = async (predicate, timeout = 2500) => {
    const start = Date.now();
    while (Date.now() - start < timeout) {
      if (await predicate()) return;
      await new Promise(resolve => setTimeout(resolve, 40));
    }
    throw Error('Native popover state did not settle before the diagnostic timeout.');
  };
  const closed = async () => pet.locator('.pet-popover').count().then(count => count === 0);
  const character = pet.getByRole('button', { name: '小柚，单击展开统计小卡，拖动移动，右键打开菜单', exact: true });
  await pet.keyboard.press('Escape');
  await settled(closed);
  await observe('initial closed');
  await character.click({ button: 'right' });
  await settled(async () => await pet.locator('.pet-menu').count() === 1);
  await observe('menu rendered before native focus check');
  await settled(async () => (await invoke(pet, 'desktop_diagnostics')).windows.some(w => w.label === 'pet' && w.focused));
  await observe('user-opened menu receives native focus');
  await invoke(main, 'open_panel');
  await settled(closed);
  const blurred = await observe('native focus transferred to main dismisses menu');
  assert.equal(blurred.petInteraction?.active, false);
  assert.ok(blurred.windows.some(w => w.label === 'main' && w.focused));
  assert.ok(blurred.windows.some(w => w.label === 'pet' && w.visible && w.activation?.noActivate));
  await character.click({ button: 'right' });
  await settled(async () => await pet.locator('.pet-menu').count() === 1);
  await pet.keyboard.press('Escape');
  await settled(closed);
  const escaped = await observe('Escape dismisses menu');
  assert.equal(escaped.petInteraction?.active, false);
  assert.ok(escaped.windows.some(w => w.label === 'pet' && w.visible && w.activation?.noActivate));
  await character.click({ button: 'right' });
  await settled(async () => await pet.locator('.pet-menu').count() === 1);
  await pet.getByRole('button', { name: '打开主面板', exact: true }).click();
  await settled(closed);
  const selected = await observe('menu action dismisses after opening main');
  assert.equal(selected.petInteraction?.active, false);
  await character.click();
  await settled(async () => await pet.locator('.pet-stats').count() === 1);
  await observe('user-opened stats card');
  await invoke(main, 'open_panel');
  await settled(closed);
  const cardBlur = await observe('native focus transferred to main dismisses card');
  assert.equal(cardBlur.petInteraction?.active, false);
  const background = await pet.evaluate(() => getComputedStyle(document.documentElement).backgroundColor);
  assert.equal(background, 'rgba(0, 0, 0, 0)');
  await pet.screenshot({ path: path.join(path.dirname(target), `${path.basename(target, '.json')}-closed.png`) });
  report.status = 'passed';
  }
} catch (error) {
  report.status = 'failed'; report.error = String(error);
  try { await failureObservation?.('failure state'); } catch { /* Retain the original failure. */ }
  process.exitCode = 1;
} finally {
  await fs.mkdir(path.dirname(target), { recursive: true });
  await fs.writeFile(target, JSON.stringify(report, null, 2) + '\n');
  await browser.close().catch(() => {});
}
process.stdout.write(JSON.stringify({ status: report.status, cases: report.cases.length,
  error: report.error, evidence: target, scope: report.scope, limitations: report.limitations }, null, 2) + '\n');
