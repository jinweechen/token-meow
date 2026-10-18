'use strict';
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const crypto = require('node:crypto');
const base = path.resolve(__dirname, '..');
const sha = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const coreSource = fs.readFileSync(path.join(base, 'mechanics-core.js'), 'utf8');
const coreContext = {};
vm.runInNewContext(coreSource, coreContext);
const model = coreContext.WalkMechanics;
const saved = JSON.parse(fs.readFileSync(path.join(base, 'mechanics.json'), 'utf8'));
const html = fs.readFileSync(path.join(base, 'mechanics.html'), 'utf8');
const inline = [...html.matchAll(/<script(?:[^>]*)>([\s\S]*?)<\/script>/g)].map(x => x[1]).filter(Boolean).join('\n');
new vm.Script(inline);
const fail = message => { throw Error(message); };
if (!saved.validation.passed) fail('Saved default numeric checks failed');
if (JSON.stringify(saved.parameters) !== JSON.stringify(model.DEFAULTS)) fail('Saved defaults differ from code');
for (let i = 0; i < model.PHASES.length; i++) {
  const p = model.PHASES[i], current = Object.assign({ keyIndex: i, name: p.name }, model.sample(p.phase * saved.parameters.cycleSeconds, saved.parameters));
  if (JSON.stringify(current) !== JSON.stringify(saved.keyframes[i])) fail('Saved keyframe differs from model: ' + i);
}
const badFoot = model.validate({ heelContactDegrees: 1, toeOffDegrees: 45, swingLift: 0.000001, hipHeight: 0.80 });
if (badFoot.passed || badFoot.checks.noSoleBelowGround) fail('Foot-penetration counterexample was incorrectly accepted');
let unreachableMessage = '';
try { model.sample(0, { hipHeight: 1.1 }); } catch (error) { unreachableMessage = error.message; }
if (!unreachableMessage.includes('Unreachable two-bone IK target; no silent limb stretching')) fail('Unreachable IK did not explicitly reject');

// This is a minimal non-browser DOM harness: it checks code paths and call counts,
// not pixels, browser rendering, responsiveness, memory use, or IAB stability.
let createdNodes = 0;
class NodeStub {
  constructor() { this.children = []; this.attrs = {}; this.style = {}; this.classList = { toggle() {} }; this.textContent = ''; this.value = ''; this.checked = false; this.hidden = true; createdNodes++; }
  setAttribute(key, value) { this.attrs[key] = String(value); }
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this.children = [...children]; }
}
const elements = new Map([...html.matchAll(/\bid="([^"]+)"/g)].map(m => [m[1], new NodeStub()]));
elements.get('sampling').value = 'continuous'; elements.get('space').value = 'inplace'; elements.get('speed').value = '1'; elements.get('trails').checked = true;
const calls = { sample: 0, validate: 0 };
const instrumented = Object.assign({}, model, {
  sample(...args) { calls.sample++; return model.sample(...args); },
  validate(...args) { calls.validate++; return model.validate(...args); }
});
const harness = vm.createContext({
  window: { WalkMechanics: instrumented },
  document: {
    hidden: false,
    getElementById(id) { if (!elements.has(id)) fail('Unknown DOM id: ' + id); return elements.get(id); },
    createElement() { return new NodeStub(); },
    createElementNS() { return new NodeStub(); },
    addEventListener() {}
  },
  fetch: async () => ({ ok: true, json: async () => saved }),
  requestAnimationFrame: () => 1, cancelAnimationFrame() {}, setTimeout,
  Blob, URL
});
async function run() {
  vm.runInContext(inline, harness);
  await new Promise(setImmediate);
  if (!elements.get('error').hidden) fail('DOM harness initialization error: ' + elements.get('error').textContent);
  if (elements.get('play').disabled) fail('Default model playback incorrectly disabled');
  const initialized = { ...calls, createdNodes, cachedPoints: vm.runInContext('cache.samples', harness) };
  if (initialized.cachedPoints !== 129 || initialized.sample !== 130 || initialized.validate !== 1) fail('Unexpected initialization sampling');
  const cachedBefore = vm.runInContext('JSON.stringify(cache)', harness);
  vm.runInContext('for (let i=0;i<24;i++){time=i/24*config.cycleSeconds;render();}', harness);
  const steady = { currentPoseSamples: calls.sample - initialized.sample, validationRuns: calls.validate - initialized.validate, createdDomNodes: createdNodes - initialized.createdNodes, cacheUnchanged: vm.runInContext('JSON.stringify(cache)', harness) === cachedBefore };
  if (steady.currentPoseSamples !== 24 || steady.validationRuns !== 0 || steady.createdDomNodes !== 0 || !steady.cacheUnchanged) fail('Per-frame sampling or DOM rebuilding regression');
  const svgDir = path.join(base, 'mechanics-reference-svg');
  const svgNames = fs.readdirSync(svgDir).filter(name => /^\d{2}-.*\.svg$/.test(name));
  if (svgNames.length !== 8) fail('Expected eight standalone SVG frames');
  for (const name of svgNames) {
    const source = fs.readFileSync(path.join(svgDir, name), 'utf8');
    if (!source.includes('viewBox="0 0 768 768"') || /<image\b/i.test(source)) fail('SVG canvas or raster-embed violation: ' + name);
  }
  const oldExpected = {
    'mechanics-core.js': '963f4ecc6a2143ccc9058205a593e5210be51cb8d8a8a5c56dea6aa54acb6721',
    'mechanics.html': 'c5454d1be16856eb8e4960cdef24742bc87bbf5933fd28434c64501b004c0731',
    'mechanics.json': '19ff54f3c1a625d752eb946ffe0a0f05b834427ecbba425ffa5e7001ddcb130c'
  };
  for (const [name, expected] of Object.entries(oldExpected)) if (sha(path.join(base, '..', 'walk-v8-study', name)) !== expected) fail('v8 source changed: ' + name);
  const report = {
    schemaVersion: 1, localDate: '2026-10-17', status: 'engineering-checks-passed-not-character-acceptance',
    sources: Object.fromEntries(['mechanics-core.js', 'mechanics.html', 'mechanics.json'].map(name => [name, sha(path.join(base, name))])),
    defaultNumericValidation: saved.validation,
    negativeCases: {
      insufficientSwingClearance: { correctlyRejected: !badFoot.passed && !badFoot.checks.noSoleBelowGround, minimumSoleHeight: badFoot.metrics.minimumSoleHeight, parameters: { heelContactDegrees: 1, toeOffDegrees: 45, swingLift: 0.000001, hipHeight: 0.80 } },
      unreachableLeg: { correctlyRejected: true, message: unreachableMessage, parameters: { hipHeight: 1.1 } }
    },
    nonBrowserDomHarness: { scope: 'Initialization, DOM identifier wiring and sample/node counts only; no browser visual or performance acceptance', initialized, twentyFourRenders: steady, configuredMaximumPaintHz: 30 },
    svg: { individualCount: 8, canvas: [768, 768], rasterEmbeds: 0, files: svgNames },
    v8SourceHashesPreserved: oldExpected,
    limitations: ['No browser or imagegen was called', 'IAB crash cause remains undiagnosed; no crash-fix claim', 'No character PNG acceptance or Q-style anatomy redefinition']
  };
  fs.writeFileSync(path.join(__dirname, 'validation-report.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify({ passed: true, negativeCases: report.negativeCases, nonBrowserDomHarness: report.nonBrowserDomHarness, svg: report.svg, v8SourcesPreserved: true }, null, 2));
}
run().catch(error => { console.error(error); process.exitCode = 1; });
