// Collect pinned dependency metadata and the actual local license/notice texts.
// Registry/package files are data only; no dependency scripts are executed.
import fs from 'node:fs/promises';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';

const workspace = await fs.realpath(path.resolve(import.meta.dirname, '..'));
const evidenceRoot = path.join(workspace, 'output');
await fs.mkdir(evidenceRoot, { recursive: true });
if (await fs.realpath(evidenceRoot) !== evidenceRoot) throw Error('License evidence root must not be a redirected directory.');
const finalOutput = path.join(evidenceRoot, 'distribution-licenses');
const generation = new Date().toISOString().replaceAll(/[:.]/g, '-');
const output = `${finalOutput}.staging-${generation}`;
await fs.mkdir(output);
const cargo = (...args) => execFileSync('cargo', args, {
  cwd: workspace, encoding: 'utf8', windowsHide: true, maxBuffer: 24 * 1024 * 1024,
});
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const unix = value => value.replaceAll(path.sep, '/');
const licenseName = /^(?:unlicense|licen[cs]e|copying|copyright|notice)(?:$|[._-])/i;
const metadata = JSON.parse(cargo('metadata', '--manifest-path', 'src-tauri/Cargo.toml',
  '--format-version', '1', '--locked', '--offline', '--filter-platform', 'x86_64-pc-windows-msvc',
  '--features', 'custom-protocol'));
const tree = cargo('tree', '--manifest-path', 'src-tauri/Cargo.toml', '--locked', '--offline',
  '--target', 'x86_64-pc-windows-msvc', '--features', 'custom-protocol', '--edges', 'normal',
  '--prefix', 'none', '--format', '{p}');
const normal = new Set(tree.split(/\r?\n/).flatMap(line => {
  const match = /^([A-Za-z0-9_-]+) v([^\s]+)/.exec(line);
  return match ? [`${match[1]}@${match[2]}`] : [];
}));
const unresolved = [];
const overrideRoot = path.join(workspace, 'third-party', 'license-overrides');
const downloads = JSON.parse((await fs.readFile(path.join(overrideRoot, 'downloads.json'), 'utf8')).replace(/^\uFEFF/, ''));
const overrides = {
  'alloc-stdlib@0.2.4': ['ae42d22078b98549e987d2f03d12df7b984fde47', ['alloc-stdlib-0.2.4-LICENSE']],
  'selectors@0.36.1': ['635e1a19d02960588a00e189bd4bd5bdb150ec3d', ['MPL-2.0.txt']],
  'unic-char-property@0.9.0': ['5878605364af97a3358368a6eaef02104af2e016', ['unic-5878605-LICENSE-MIT', 'unic-5878605-LICENSE-APACHE']],
  'unic-char-range@0.9.0': ['5878605364af97a3358368a6eaef02104af2e016', ['unic-5878605-LICENSE-MIT', 'unic-5878605-LICENSE-APACHE']],
  'unic-common@0.9.0': ['5878605364af97a3358368a6eaef02104af2e016', ['unic-5878605-LICENSE-MIT', 'unic-5878605-LICENSE-APACHE']],
  'unic-ucd-ident@0.9.0': ['8a6ce83063d90b91ae2ce59eddb803edd393fca9', ['unic-8a6ce83-LICENSE-MIT', 'unic-8a6ce83-LICENSE-APACHE']],
  'unic-ucd-version@0.9.0': ['5878605364af97a3358368a6eaef02104af2e016', ['unic-5878605-LICENSE-MIT', 'unic-5878605-LICENSE-APACHE']],
  'webview2-com@0.38.2': ['b74dc5e2b394044bea5191052868ce7a106c202c', ['webview2-b74dc5e-LICENSE']],
  'webview2-com-sys@0.38.2': ['b74dc5e2b394044bea5191052868ce7a106c202c', ['webview2-b74dc5e-LICENSE']],
  'webview2-com-macros@0.8.1': ['dffa41a8a46d3f5565eefbff2de57d38d399f158', ['webview2-dffa41a-LICENSE']],
};
const cargoLock = await fs.readFile(path.join(workspace, 'src-tauri', 'Cargo.lock'), 'utf8');
const checksums = new Map(cargoLock.split('[[package]]').flatMap(block => {
  const name = /^name = "([^"]+)"$/m.exec(block)?.[1];
  const version = /^version = "([^"]+)"$/m.exec(block)?.[1];
  const checksum = /^checksum = "([a-f0-9]{64})"$/m.exec(block)?.[1];
  return name && version && checksum ? [[`${name}@${version}`, checksum]] : [];
}));

async function collectLicenseTexts(root, directory, explicitFile) {
  const candidates = new Set();
  async function visit(folder, depth) {
    for (const entry of await fs.readdir(folder, { withFileTypes: true })) {
      if (entry.isSymbolicLink()) continue;
      const file = path.join(folder, entry.name);
      if (entry.isFile() && licenseName.test(entry.name)) candidates.add(file);
      else if (entry.isDirectory() && depth < 2 && /^(?:licenses?|notices?)(?:$|[._-])/i.test(entry.name)) {
        await visit(file, depth + 1);
      }
    }
  }
  await visit(root, 0);
  if (explicitFile) {
    const file = path.resolve(root, explicitFile);
    if (file.startsWith(root + path.sep)) candidates.add(file);
  }
  const records = [];
  for (const source of [...candidates].sort()) {
    const real = await fs.realpath(source);
    if (!real.startsWith(root + path.sep)) throw Error('A license path escaped its package.');
    const bytes = await fs.readFile(real);
    if (bytes.length > 2 * 1024 * 1024) throw Error('License text exceeds the collection bound.');
    const relative = path.relative(root, source);
    const destination = path.join(output, 'texts', directory, relative);
    await fs.mkdir(path.dirname(destination), { recursive: true });
    await fs.writeFile(destination, bytes);
    records.push({ source: unix(relative), file: unix(path.relative(output, destination)),
      bytes: bytes.length, sha256: digest(bytes) });
  }
  return records;
}

async function copyOverrides(pkg, root) {
  const override = overrides[`${pkg.name}@${pkg.version}`];
  if (!override) return [];
  const vcs = JSON.parse(await fs.readFile(path.join(root, '.cargo_vcs_info.json'), 'utf8'));
  if (vcs.git.sha1 !== override[0]) throw Error(`Pinned license commit mismatch: ${pkg.name}`);
  const records = [];
  for (const name of override[1]) {
    const origin = downloads.find(item => item.file === name);
    if (!origin) throw Error(`Missing pinned license download: ${name}`);
    const bytes = await fs.readFile(path.join(overrideRoot, name));
    if (digest(bytes) !== origin.sha256) throw Error(`Pinned license digest mismatch: ${name}`);
    const destination = path.join(output, 'texts', `cargo-${pkg.name}-${pkg.version}`, 'upstream', name);
    await fs.mkdir(path.dirname(destination), { recursive: true });
    await fs.writeFile(destination, bytes);
    records.push({ source: origin.url, repositoryCommit: override[0],
      file: unix(path.relative(output, destination)), bytes: bytes.length, sha256: digest(bytes) });
  }
  return records;
}

async function copySourceArchive(pkg, root) {
  const archive = `${pkg.name}-${pkg.version}.crate`;
  const registry = path.basename(path.dirname(root));
  const cached = path.resolve(root, '..', '..', '..', 'cache', registry, archive);
  const bytes = await fs.readFile(cached);
  if (digest(bytes) !== checksums.get(`${pkg.name}@${pkg.version}`)) throw Error(`Source archive differs from Cargo.lock: ${archive}`);
  const destination = path.join(output, 'sources', archive);
  await fs.mkdir(path.dirname(destination), { recursive: true });
  await fs.writeFile(destination, bytes);
  return { file: unix(path.relative(output, destination)), bytes: bytes.length, sha256: digest(bytes),
    source: `https://static.crates.io/crates/${pkg.name}/${archive}`, modified: false };
}

const rust = [];
for (const pkg of metadata.packages.filter(pkg => normal.has(`${pkg.name}@${pkg.version}`) && pkg.source).sort((a, b) => `${a.name}@${a.version}`.localeCompare(`${b.name}@${b.version}`))) {
  const root = await fs.realpath(path.dirname(pkg.manifest_path));
  const texts = [...await collectLicenseTexts(root, `cargo-${pkg.name}-${pkg.version}`, pkg.license_file),
    ...await copyOverrides(pkg, root)];
  // Ship the exact source for MPL components, and UNIC's omitted copyright headers.
  const sourceArchive = /MPL-2\.0/.test(pkg.license ?? '') || pkg.name.startsWith('unic-')
    ? await copySourceArchive(pkg, root) : undefined;
  if (!pkg.license || texts.length === 0) unresolved.push(`cargo:${pkg.name}@${pkg.version}`);
  rust.push({ name: pkg.name, version: pkg.version, license: pkg.license, authors: pkg.authors,
    repository: pkg.repository, source: pkg.source, licenseTexts: texts, sourceArchive });
}

const project = JSON.parse(await fs.readFile(path.join(workspace, 'package.json'), 'utf8'));
const npm = [], visited = new Set();
async function npmPackage(name, from) {
  const require = createRequire(path.join(from, 'package.json'));
  const entry = require.resolve(name);
  let root = path.dirname(await fs.realpath(entry));
  let pkg;
  while (root !== path.dirname(root)) {
    try {
      const candidate = JSON.parse(await fs.readFile(path.join(root, 'package.json'), 'utf8'));
      if (candidate.name === name) { pkg = candidate; break; }
    } catch (error) { if (error.code !== 'ENOENT') throw error; }
    root = path.dirname(root);
  }
  if (!pkg) throw Error(`Cannot locate installed package metadata: ${name}`);
  const identity = `${pkg.name}@${pkg.version}`;
  if (visited.has(identity)) return;
  visited.add(identity);
  const texts = await collectLicenseTexts(root, `npm-${pkg.name.replaceAll('/', '-')}-${pkg.version}`);
  if (!pkg.license || texts.length === 0) unresolved.push(`npm:${identity}`);
  npm.push({ name: pkg.name, version: pkg.version, license: pkg.license,
    repository: pkg.repository, licenseTexts: texts });
  for (const dependency of Object.keys(pkg.dependencies ?? {}).sort()) await npmPackage(dependency, root);
}
for (const name of Object.keys(project.dependencies).sort()) await npmPackage(name, workspace);
npm.sort((a, b) => a.name.localeCompare(b.name));

const assetManifestPath = 'assets/pets/nekomi/manifest-animation-v11.json';
const assetPath = path.join(workspace, assetManifestPath);
const asset = JSON.parse(await fs.readFile(assetPath, 'utf8'));
const clipIds = ['idle', 'active', 'alert', 'complete', 'happy', 'wink',
  'affection', 'shy', 'bow', 'stretch', 'hug-tail', 'sleep', 'walk'];
const assetSourceRoot = 'assets/pets/nekomi/';
const animationSourceRoot = `${assetSourceRoot}walk-v10-complete/`;
const retainedAnimationRoot = `${assetSourceRoot}animation-v6/`;
if (asset.schema_version !== 2 || asset.skin_id !== 'nekomi-animation-v11'
    || asset.renderer_kind !== 'builtin-animation-clips' || !Array.isArray(asset.frames)
    || asset.frames.length !== 52 || new Set(asset.frames.map(frame => frame.id)).size !== 52
    || !Array.isArray(asset.clips) || asset.clips.length !== 13
    || JSON.stringify(asset.clips.map(clip => clip.id)) !== JSON.stringify(clipIds)
    || asset.license !== 'LicenseRef-Project-Owned'
    || asset.bounds_alpha_threshold !== 127) throw Error('Review the v11 animation clip manifest.');
const assetSourceManifestPath = `${animationSourceRoot}catalog-v11.json`;
const assetSourceManifestBytes = await fs.readFile(path.join(workspace, assetSourceManifestPath));
if (asset.source_archive?.manifest?.path !== assetSourceManifestPath
    || asset.source_archive.manifest.sha256 !== digest(assetSourceManifestBytes)) {
  throw Error('Animation source manifest differs from the runtime source binding.');
}
const assetSource = JSON.parse(assetSourceManifestBytes.toString('utf8'));
const deliveryPath = `${animationSourceRoot}delivery-manifest-v11.json`;
const delivery = JSON.parse(await fs.readFile(path.join(workspace, deliveryPath), 'utf8'));
if (assetSource.schemaVersion !== 1 || JSON.stringify(assetSource.canvas) !== '[1024,1536]'
    || !Array.isArray(assetSource.clips)
    || JSON.stringify(assetSource.clips.map(clip => clip.id)) !== JSON.stringify(clipIds)
    || delivery.schemaVersion !== 1 || delivery.status !== 'baseline-assets-delivered-with-known-variations'
    || !Array.isArray(delivery.items)) throw Error('Review the delivered v11 animation source pack.');
// The source catalog deliberately reuses v6 PNGs through ../animation-v6 paths.
// Normalize those finite source references into canonical project-relative paths;
// never allow catalog data to supply arbitrary files to the distributor.
function sourceFramePath(sourcePath) {
  if (typeof sourcePath !== 'string' || !/^(?:\.\.\/)?[a-z0-9][a-z0-9/_-]*\.png$/.test(sourcePath)) {
    throw Error('Animation source frame path is not a controlled PNG reference.');
  }
  const normalized = path.posix.normalize(`${animationSourceRoot}${sourcePath}`);
  if (!normalized.startsWith(animationSourceRoot) && !normalized.startsWith(`${retainedAnimationRoot}clips/`)) {
    throw Error('Animation source frame escaped its two controlled source directories.');
  }
  return normalized;
}
const sourceFrames = new Map();
for (const clip of assetSource.clips) {
  if (!Array.isArray(clip.frames) || !clip.frames.length || clip.frames.length > 64) {
    throw Error('Review the finite animation source sequence.');
  }
  for (const source of clip.frames) {
    const sourcePath = sourceFramePath(source.path);
    const id = sourcePath.slice(assetSourceRoot.length, -4);
    if (source.exists !== true || !/^[a-f0-9]{64}$/.test(source.sha256)
        || (sourceFrames.has(id) && sourceFrames.get(id).sha256 !== source.sha256)) {
      throw Error('Animation source reuses a frame with inconsistent provenance.');
    }
    sourceFrames.set(id, { ...source, path: sourcePath });
  }
}
if (sourceFrames.size !== 52) throw Error('Review the v11 animation source frame count.');
const frontFrame = 'walk-v10-complete/model-reference/master-front-v2';
for (const clip of asset.clips) {
  const source = assetSource.clips.find(candidate => candidate.id === clip.id);
  const expectedSequence = source.frames.map(step => ({
    frame: sourceFramePath(step.path).slice(assetSourceRoot.length, -4),
    durationMs: step.durationMs, segment: step.segment,
  }));
  const loops = ['idle', 'active', 'alert'].includes(clip.id);
  const bodyStart = source.frames.findIndex(step => step.segment === 'body');
  const bodyEnd = source.frames.findLastIndex(step => step.segment === 'body') + 1;
  const fallback = clip.id === 'sleep' ? 'walk-v10-complete/actions-v11/sleep/seated'
    : ['active', 'alert'].includes(clip.id) ? `animation-v6/clips/${clip.id}/base` : frontFrame;
  if (!sourceFrames.has(fallback) || clip.fallback !== fallback || clip.loop !== loops
      || JSON.stringify(clip.canvas) !== '[1024,1536]'
      || clip.sharedTransform?.scale !== 1 || JSON.stringify(clip.sharedTransform?.anchor) !== '[512,1490]'
      || JSON.stringify(clip.sharedTransform?.translate) !== '[0,0]'
      || !Array.isArray(clip.sequence) || clip.sequence.length < 1 || clip.sequence.length > 64
      || clip.sequence.some(step => !sourceFrames.has(step.frame)
        || !Number.isSafeInteger(step.durationMs) || step.durationMs < 16 || step.durationMs > 10000
        || !['entry', 'body', 'exit', 'returnToIdle'].includes(step.segment))
      || JSON.stringify(clip.sequence) !== JSON.stringify(expectedSequence)
      || new Set([clip.fallback, ...clip.sequence.map(step => step.frame)]).size > 12
      || (loops ? JSON.stringify(clip.loopRange) !== JSON.stringify({ start: bodyStart, end: bodyEnd })
        || bodyStart < 0 || bodyEnd <= bodyStart
        || source.frames.slice(bodyStart, bodyEnd).some(step => step.segment !== 'body')
        : clip.loopRange !== undefined)) {
    throw Error('Animation clip differs from its reviewed fixed-canvas sequence.');
  }
}
const files = [];
const assetRoot = path.join(workspace, 'assets', 'pets', 'nekomi');
if (await fs.realpath(assetRoot) !== assetRoot) throw Error('Animation clip root must not be redirected.');
const deliveredFrames = new Map(delivery.items.map(item => [item.path, item]));
for (const resource of asset.frames) {
  const source = sourceFrames.get(resource.id);
  const delivered = deliveredFrames.get(resource.path?.slice(assetSourceRoot.length));
  if (!source || resource.path !== `${assetSourceRoot}${resource.id}.png`
      || resource.path !== source.path
      || resource.width !== 1024 || resource.height !== 1536
      || !Number.isSafeInteger(resource.bytes) || resource.bytes < 45 || resource.bytes > 4 * 1024 * 1024
      || !/^[a-f0-9]{64}$/.test(resource.sha256) || source.sha256 !== resource.sha256
      || !delivered || delivered.sha256 !== resource.sha256 || delivered.bytes !== resource.bytes
      || JSON.stringify(delivered.canvas) !== '[1024,1536]' || delivered.mode !== 'RGBA'
      || JSON.stringify(delivered.visibleBounds127) !== JSON.stringify(resource.bounds)
      || !Array.isArray(resource.bounds) || resource.bounds.length !== 4
      || resource.bounds.some(value => !Number.isSafeInteger(value))
      || resource.bounds[0] < 0 || resource.bounds[1] < 0
      || resource.bounds[2] <= resource.bounds[0] || resource.bounds[3] <= resource.bounds[1]
      || resource.bounds[2] > resource.width || resource.bounds[3] > resource.height) {
    throw Error('Animation frame differs from its reviewed controlled PNG definition.');
  }
  const file = path.resolve(workspace, resource.path);
  if (!file.startsWith(assetRoot + path.sep) || path.extname(file) !== '.png'
      || await fs.realpath(file) !== file) throw Error('Animation frame escaped its controlled PNG directory.');
  const bytes = await fs.readFile(file);
  if (digest(bytes) !== resource.sha256 || bytes.length !== resource.bytes
      || !bytes.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))
      || bytes.toString('ascii', 12, 16) !== 'IHDR'
      || bytes.readUInt32BE(16) !== 1024 || bytes.readUInt32BE(20) !== 1536
      || bytes[24] !== 8 || bytes[25] !== 6) throw Error('Pet asset differs from its original RGBA PNG manifest.');
  files.push({ path: unix(path.relative(workspace, file)), bytes: bytes.length, sha256: digest(bytes) });
}
if (files.reduce((sum, file) => sum + file.bytes, 0) > 128 * 1024 * 1024) {
  throw Error('Animation asset pack exceeds the 128 MiB encoded-byte budget.');
}
const icons = [];
for (const file of ['public/favicon.svg', 'src-tauri/icons/source.svg', 'src-tauri/icons/32x32.png',
  'src-tauri/icons/128x128.png', 'src-tauri/icons/icon.png', 'src-tauri/icons/icon.ico']) {
  const bytes = await fs.readFile(path.join(workspace, file));
  icons.push({ path: file, bytes: bytes.length, sha256: digest(bytes),
    author: 'Token 喵项目原创几何绘图', license: 'LicenseRef-Project-Owned',
    source: file.startsWith('public/') ? 'editable original SVG; no external artwork or fonts'
      : 'src-tauri/icons/source.svg; reproducible with scripts/generate-icons.ps1' });
}
const assetProvenance = [];
const sourceRecordRoot = 'output/imagegen/completion-20261018/';
const sourceRecordDocuments = ['master', 'walk-key', 'walk-fill', 'actions-a',
  'actions-b', 'actions-c', 'actions-d', 'actions-e']
  .map(name => `${sourceRecordRoot}source-records-${name}.json`);
const requiredGenerationDocuments = [deliveryPath, `${animationSourceRoot}generation-manifest-v2.json`,
  `${retainedAnimationRoot}generation-root.json`, `${retainedAnimationRoot}generation-cute.json`,
  ...sourceRecordDocuments];
const assetSourceDocuments = [assetManifestPath, assetSourceManifestPath, deliveryPath,
  ...['DELIVERY-20261018.txt', 'generation-manifest-v2.json', 'sequence-v11.json'].map(file => `${animationSourceRoot}${file}`),
  ...['README.txt', 'generation-root.json', 'generation-cute.json', 'generation-cute-sources.json',
    'generation-cute-planned.json', 'root-specs.json'].map(file => `${retainedAnimationRoot}${file}`),
  ...sourceRecordDocuments];
// Copy finite authored text directories, never recursively copy PNGs, layers or previews.
for (const folder of [`${animationSourceRoot}prompts`, `${retainedAnimationRoot}prompts/root`, `${retainedAnimationRoot}prompts/cute`,
  `${assetSourceRoot}hd-v5-natural-tail/prompts`]) {
  for (const entry of (await fs.readdir(path.join(workspace, folder), { withFileTypes: true }))
    .filter(entry => entry.isFile() && /^[a-z0-9-]+\.txt$/.test(entry.name)).sort((a, b) => a.name.localeCompare(b.name))) {
    assetSourceDocuments.push(`${folder}/${entry.name}`);
  }
}
// Record existing base-pack documents only, never recursively copy its PNGs or rejected drafts.
for (const name of ['manifest.json', 'generation-root.json', 'generation-extra.json', 'generation-cute.json',
  'image-analysis.json', 'visual-review.json', 'README.md', 'README.txt']) {
  const file = `${assetSourceRoot}hd-v5-natural-tail/${name}`;
  try {
    const entry = await fs.lstat(path.join(workspace, file));
    if (!entry.isFile() || entry.isSymbolicLink()) throw Error('Base-pack provenance must be an ordinary document.');
    assetSourceDocuments.push(file);
  } catch (error) { if (error.code !== 'ENOENT') throw error; }
}
if (!Array.isArray(asset.source_archive.generation)
    || new Set(asset.source_archive.generation.map(record => record.path)).size !== asset.source_archive.generation.length
    || requiredGenerationDocuments.some(file => !asset.source_archive.generation.some(record => record.path === file))) {
  throw Error('Animation catalog omitted the finite new and retained generation provenance.');
}
for (const record of asset.source_archive.generation) {
  if (!assetSourceDocuments.includes(record.path)
      || digest(await fs.readFile(path.join(workspace, record.path))) !== record.sha256) {
    throw Error('Animation generation source differs from its runtime binding.');
  }
}
for (const file of [...new Set(assetSourceDocuments)]) {
  const origin = path.join(workspace, file);
  const isSourceRecord = sourceRecordDocuments.includes(file);
  if ((!file.startsWith(assetSourceRoot) && !isSourceRecord) || !/\.(?:json|txt|md)$/.test(file)
      || await fs.realpath(origin) !== origin) throw Error('Animation provenance escaped its finite document list.');
  const bytes = await fs.readFile(origin);
  if (bytes.length > 2 * 1024 * 1024) throw Error('Animation provenance document exceeds the collection bound.');
  const destination = path.join(output, 'assets', isSourceRecord
    ? `source-records/${path.posix.basename(file)}` : file.slice(assetSourceRoot.length));
  await fs.mkdir(path.dirname(destination), { recursive: true });
  await fs.writeFile(destination, bytes);
  assetProvenance.push({ source: file, file: unix(path.relative(output, destination)), bytes: bytes.length, sha256: digest(bytes) });
}
const loaderPackage = metadata.packages.find(pkg => pkg.name === 'webview2-com-sys' && pkg.version === '0.38.2');
if (!loaderPackage) throw Error('Review the native loader notice when its pinned package changes.');
const loaderLicense = downloads.find(item => item.file === 'WebView2Loader-1.0.3650.58-LICENSE.txt');
if (!loaderLicense) throw Error('Native loader redistribution notice is missing.');
const loaderLicenseBytes = await fs.readFile(path.join(overrideRoot, loaderLicense.file));
if (digest(loaderLicenseBytes) !== loaderLicense.sha256) throw Error('Native loader notice digest mismatch.');
await fs.mkdir(path.join(output, 'texts', 'native-WebView2Loader-1.0.3650.58'), { recursive: true });
const loaderLicenseFile = `texts/native-WebView2Loader-1.0.3650.58/${loaderLicense.file}`;
await fs.writeFile(path.join(output, loaderLicenseFile), loaderLicenseBytes);
const loaderFiles = [];
for (const file of ['WebView2Loader.dll', 'WebView2Loader.dll.lib', 'WebView2LoaderStatic.lib']) {
  const bytes = await fs.readFile(path.join(path.dirname(loaderPackage.manifest_path), 'x64', file));
  loaderFiles.push({ path: `webview2-com-sys-0.38.2/x64/${file}`, bytes: bytes.length, sha256: digest(bytes) });
}
const cliRoot = await fs.realpath(path.join(workspace, 'node_modules', '@tauri-apps', 'cli'));
const cliPackage = JSON.parse(await fs.readFile(path.join(cliRoot, 'package.json'), 'utf8'));
if (cliPackage.version !== '2.12.3') throw Error('Review the pinned installer template when the Tauri CLI changes.');
const templateBytes = await fs.readFile(path.join(workspace, 'src-tauri', 'windows', 'installer.nsi'));
const installerTools = [{ name: '@tauri-apps/cli installer template', version: cliPackage.version,
  license: cliPackage.license, licenseTexts: await collectLicenseTexts(cliRoot, 'installer-tauri-cli-2.12.3'),
  upstreamSource: 'https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.3/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi',
  upstreamSha256: '8ae02c66f8adb4a0d15d97f984fbacd2b44262d8a4d03fefaf6177f67f7008e5',
  localSource: 'src-tauri/windows/installer.nsi', localSha256: digest(templateBytes),
  modification: 'Source/license header and exact-owned Run entry condition for ordinary uninstall; all other upstream behavior retained.' }];
const nsisRoot = path.join(process.env.LOCALAPPDATA ?? '', 'tauri', 'NSIS');
const nsisVersion = execFileSync(path.join(nsisRoot, 'makensis.exe'), ['/VERSION'], { encoding: 'utf8', windowsHide: true }).trim();
if (nsisVersion !== 'v3.13') throw Error('Review installer notices for a different NSIS version.');
const nsisNotice = await fs.readFile(path.join(nsisRoot, 'COPYING'));
if (digest(nsisNotice) !== '388357c1215ff403c5ebde3a5ecd273e68f8b79a579996775245d1ee65442aba')
  throw Error('The reviewed NSIS 3.13 notice changed.');
const nsisNoticeFile = 'texts/installer-NSIS-3.13/COPYING';
await fs.mkdir(path.dirname(path.join(output, nsisNoticeFile)), { recursive: true });
await fs.writeFile(path.join(output, nsisNoticeFile), nsisNotice);
installerTools.push({ name: 'NSIS', version: nsisVersion, license: 'See COPYING: zlib/libpng, bzip2, CPL-1.0 and stated LZMA linking exception',
  licenseTexts: [{ source: 'COPYING from the pinned local Tauri NSIS tool distribution', file: nsisNoticeFile,
    bytes: nsisNotice.length, sha256: digest(nsisNotice) }],
  scope: 'Compiler distribution and installer stub/compression notices; no NSIS source modifications.' });
const pluginFile = path.join(nsisRoot, 'Plugins', 'x86-unicode', 'additional', 'nsis_tauri_utils.dll');
const pluginBytes = await fs.readFile(pluginFile);
if (digest(pluginBytes) !== '5ba143b5db4a87d32d6e7802e033330aae56cbceabe0d1e3ba41948385ad4709')
  throw Error('Review the installer plugin notice when its binary changes.');
const pluginLicenseTexts = [];
for (const file of ['nsis-utils-0.5.3-LICENSE-MIT', 'nsis-utils-0.5.3-LICENSE-APACHE-2.0']) {
  const notice = downloads.find(item => item.file === file);
  if (!notice || notice.commit !== '13d9edd27b69310e108d6fbd49f90992f8a05390')
    throw Error('Pinned installer plugin source notice is missing.');
  const bytes = await fs.readFile(path.join(overrideRoot, file));
  if (digest(bytes) !== notice.sha256) throw Error('Installer plugin notice digest mismatch.');
  const destination = `texts/installer-nsis_tauri_utils-0.5.3/${file}`;
  await fs.mkdir(path.dirname(path.join(output, destination)), { recursive: true });
  await fs.writeFile(path.join(output, destination), bytes);
  pluginLicenseTexts.push({ source: notice.url, file: destination, bytes: bytes.length, sha256: digest(bytes) });
}
installerTools.push({ name: 'nsis_tauri_utils', version: '0.5.3', license: 'MIT OR Apache-2.0',
  sourceCommit: '13d9edd27b69310e108d6fbd49f90992f8a05390',
  release: 'https://github.com/tauri-apps/nsis-tauri-utils/releases/tag/nsis_tauri_utils-v0.5.3',
  binary: { path: 'NSIS/Plugins/x86-unicode/additional/nsis_tauri_utils.dll', bytes: pluginBytes.length,
    sha256: digest(pluginBytes), sha1: createHash('sha1').update(pluginBytes).digest('hex') },
  licenseTexts: pluginLicenseTexts, scope: 'Unmodified official installer plugin distributed in the NSIS setup.' });
const lockfiles = [];
for (const file of ['pnpm-lock.yaml', 'src-tauri/Cargo.lock']) {
  const bytes = await fs.readFile(path.join(workspace, file));
  lockfiles.push({ path: file, sha256: digest(bytes) });
}
const manifest = {
  generatedAt: new Date().toISOString(), product: 'TokenMeow', version: project.version,
  target: 'x86_64-pc-windows-msvc', projectLicense: 'UNSELECTED; no open-source license is assigned by this inventory',
  method: 'Windows Cargo normal-edge dependency graph with custom-protocol; installed npm production dependency closure; actual local notice/license files copied byte-for-byte',
  limitations: [
    'The Cargo graph includes compile-time procedural macros and may exceed the final linked subset.',
    'Build/dev-only and other-platform dependency graphs are excluded.',
    'Package SPDX metadata does not replace the copied license and notice texts.',
    'WebView2, Windows and Visual C++ runtime components follow their own installed component terms.',
  ],
  lockfiles, rust, npm, installerTools,
  assets: { manifest: assetManifestPath, manifestSha256: digest(await fs.readFile(assetPath)),
    rendererKind: asset.renderer_kind, author: asset.author, source: asset.source, license: asset.license, attribution: asset.attribution, files, icons, provenance: assetProvenance },
  nativeComponents: [{ name: 'Microsoft WebView2Loader', sdkVersion: '1.0.3650.58',
    license: 'BSD-3-Clause', licenseSource: loaderLicense.url,
    licenseText: { file: loaderLicenseFile, bytes: loaderLicenseBytes.length, sha256: digest(loaderLicenseBytes) },
    files: loaderFiles, scope: 'Loader files shipped by the exact webview2-com-sys crate; the separately installed WebView2 runtime is not bundled.' }],
  unresolved,
};
await fs.writeFile(path.join(output, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
await fs.writeFile(path.join(output, 'THIRD-PARTY-NOTICES.txt'), [
  `TokenMeow ${project.version} — Windows x64 dependency and original asset inventory`,
  'Project code license: not selected. Original pet assets: see manifest.json; third-party reuse is not granted.',
  'This inventory preserves dependency notices independently of the project and asset licenses.',
  'Full source license texts are under texts/. Package metadata and SHA-256 are in manifest.json.',
  'The exact unmodified MPL component source archives are under sources/; extract the .crate gzip tar archives to obtain their source code and original notices.',
  'UNIC source archives preserve the copyright headers omitted from its published root license files.',
  'Microsoft WebView2Loader SDK redistribution notice is included separately from the Rust wrapper MIT license.',
  'The pinned Tauri installer template and NSIS stub/compression notices are listed under installerTools.',
  '', ...rust.map(pkg => `Cargo ${pkg.name} ${pkg.version}: ${pkg.license ?? 'UNRESOLVED'}`),
  '', ...npm.map(pkg => `npm ${pkg.name} ${pkg.version}: ${pkg.license ?? 'UNRESOLVED'}`),
  '', `Unresolved metadata/texts: ${unresolved.length}`, '',
].join('\n'));
if (unresolved.length === 0) {
  // Replace only our generated artifact directory, preserving the preceding generation.
  // Both absolute rename targets stay within the verified workspace evidence root.
  try {
    const prior = await fs.lstat(finalOutput);
    if (!prior.isDirectory() || prior.isSymbolicLink() || await fs.realpath(finalOutput) !== finalOutput) {
      throw Error('Refusing to replace a redirected/non-directory license artifact.');
    }
    const history = path.join(evidenceRoot, 'license-history');
    await fs.mkdir(history, { recursive: true });
    if (await fs.realpath(history) !== history) throw Error('License history must not be redirected.');
    await fs.rename(finalOutput, path.join(history, generation));
  } catch (error) { if (error.code !== 'ENOENT') throw error; }
  await fs.rename(output, finalOutput);
} else process.exitCode = 1;
process.stdout.write(JSON.stringify({ output: unresolved.length ? output : finalOutput,
  rustPackages: rust.length, npmPackages: npm.length,
  copiedTexts: 1 + [...rust, ...npm, ...installerTools].reduce((sum, pkg) => sum + pkg.licenseTexts.length, 0),
  sourceArchives: rust.filter(pkg => pkg.sourceArchive).length, unresolved }, null, 2) + '\n');
