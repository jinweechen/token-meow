// Read-only diagnostics against this application's owned WebView2 test endpoint.
// Native input/transparent hit testing is verified separately with Computer Use.
import { createRequire } from 'node:module';
import path from 'node:path';
import fs from 'node:fs/promises';

const args = Object.fromEntries(process.argv.slice(2).map(arg => {
  const index = arg.indexOf('=');
  return [arg.slice(0, index < 0 ? undefined : index).replace(/^--/, ''), index < 0 ? true : arg.slice(index + 1)];
}));
const endpoint = new URL(String(args.cdp ?? 'http://127.0.0.1:9222'));
if (!['127.0.0.1', 'localhost', '[::1]'].includes(endpoint.hostname) || endpoint.protocol !== 'http:') throw Error('Only the owned loopback WebView2 endpoint is accepted.');
const require = createRequire(import.meta.url);
const modulePath = process.env.TOKEN_MEOW_TEST_NODE_MODULES;
const { chromium } = require(modulePath ? path.join(modulePath, 'playwright') : 'playwright');
const browser = await chromium.connectOverCDP(endpoint.toString());
try {
  const pages = browser.contexts().flatMap(context => context.pages()).filter(page => {
    const url = new URL(page.url());
    return ['tauri.localhost', 'localhost', '127.0.0.1'].includes(url.hostname);
  });
  const which = String(args.window ?? 'main');
  const selected = pages.filter(page => new URL(page.url()).searchParams.get('window') === 'pet' ? which === 'pet' : which === 'main');
  if (selected.length !== 1) throw Error(`Expected one ${which} window, found ${selected.length}.`);
  const page = selected[0];
  const allowed = ['summary', 'diagnostics', 'metrics', 'render'];
  const operation = String(args.operation ?? 'summary');
  if (!allowed.includes(operation)) throw Error('Only read-only diagnostic operations are accepted.');
  const command = operation === 'summary' ? 'get_snapshot' : operation === 'diagnostics' ? 'desktop_diagnostics' : 'desktop_metrics';
  const result = operation === 'render' ? await page.evaluate(() => {
    const styles = {};
    for (const [name, node] of [['html', document.documentElement], ['body', document.body], ['root', document.getElementById('root')]]) {
      const style = getComputedStyle(node);
      styles[name] = { backgroundColor: style.backgroundColor, opacity: style.opacity };
    }
    const rect = node => { const r = node?.getBoundingClientRect(); return r ? { x: r.x, y: r.y, width: r.width, height: r.height } : null; };
    return {
      url: location.origin + location.pathname + location.search,
      styles, viewport: { width: innerWidth, height: innerHeight },
      pet: { rectangle: rect(document.querySelector('.pet-window')), character: rect(document.querySelector('.character')),
        className: document.querySelector('.character')?.className,
        svgCount: document.querySelectorAll('.character svg').length,
        runningAnimations: document.querySelector('.character')?.getAnimations({ subtree: true }).filter(a => a.playState === 'running').length,
        compiledFallback: !!document.querySelector('.character > svg[aria-label="小柚静态兜底"]'),
        raster: (() => {
          const node = document.querySelector('.pet-sprite');
          if (!node) return null;
          const style = getComputedStyle(node);
          return { frame: node.getAttribute('data-frame'), rectangle: rect(node),
            backgroundSize: style.backgroundSize, backgroundPosition: style.backgroundPosition,
            controlledBlob: style.backgroundImage.startsWith('url("blob:') };
        })(),
      },
    };
  }) : await page.evaluate(async ({ command }) => {
    if (!window.__TAURI_INTERNALS__?.invoke) throw Error('This page is not the native Token Meow application.');
    const value = await window.__TAURI_INTERNALS__.invoke(command, command === 'get_snapshot' ? { query: {} } : {});
    if (command !== 'get_snapshot') return value;
    return {
      overview: value.overview,
      counts: Object.fromEntries(['sessions', 'turns', 'tools', 'inbox', 'sources'].map(key => [key, Array.isArray(value[key]) ? value[key].length : null])),
      activity: value.activity,
      health: { lastSuccessfulScan: value.health?.lastSuccessfulScan, lastSourceEvent: value.health?.lastSourceEvent, files: value.health?.files, pendingFiles: value.health?.pendingFiles, coverageFrom: value.health?.coverageFrom, coverageTo: value.health?.coverageTo, issueCount: Array.isArray(value.health?.issues) ? value.health.issues.length : value.health?.issues, conflictCount: Array.isArray(value.health?.conflicts) ? value.health.conflicts.length : value.health?.conflicts },
      account: value.account ? { status: value.account.status, collectedAt: value.account.collectedAt, buckets: value.account.buckets?.length, accountScope: value.account.accountScope, sourceId: value.account.sourceId, error: value.account.error } : null,
      capabilities: value.capabilities,
    };
  }, { command });
  if (args.screenshot) {
    if (operation !== 'render') throw Error('Screenshots are only available with the read-only render inspection.');
    const screenshot = path.resolve(String(args.screenshot));
    if (!screenshot.startsWith(path.resolve('output') + path.sep) || path.extname(screenshot).toLowerCase() !== '.png') throw Error('Screenshot must be a workspace output PNG.');
    await fs.mkdir(path.dirname(screenshot), { recursive: true });
    // Keep the WebView's actual background. Do not force an alpha override.
    await page.screenshot({ path: screenshot });
    result.screenshot = screenshot;
    result.screenshotBackgroundOverride = false;
  }
  const output = JSON.stringify({ observedAt: new Date().toISOString(), window: which, operation, result }, null, 2);
  if (args.out) {
    const outputRoot = path.resolve('output');
    const target = path.resolve(String(args.out));
    if (!target.startsWith(outputRoot + path.sep)) throw Error('Evidence must remain inside the workspace output directory.');
    await fs.mkdir(path.dirname(target), { recursive: true });
    await fs.writeFile(target, output + '\n');
  }
  process.stdout.write(output + '\n');
} finally {
  // Disconnect the test client without closing the user's native application windows.
  await browser.close();
}
