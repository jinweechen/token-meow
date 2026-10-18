// Create bounded, content-free native fixtures and an independent counting oracle.
import fs from 'node:fs/promises';
import { createReadStream } from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';

const options = Object.fromEntries(process.argv.slice(2).map(arg => {
  const split = arg.indexOf('='); return [arg.slice(2, split), arg.slice(split + 1)];
}));
const source = await fs.realpath(options.source ?? process.env.CODEX_HOME ?? path.join(process.env.USERPROFILE, '.codex'));
const target = path.resolve(options.out ?? `output/local-redacted-${Date.now()}`);
if (target.toLowerCase().startsWith(source.toLowerCase() + path.sep) || target.toLowerCase() === source.toLowerCase()) throw Error('Fixtures must not be written in the source root.');
await fs.mkdir(target, { recursive: true });
if ((await fs.readdir(target)).length) throw Error('Evidence output must be a new empty directory.');
const candidates = [];
async function discover(directory) {
  for (const entry of await fs.readdir(directory, { withFileTypes: true }).catch(() => [])) {
    if (entry.isSymbolicLink()) continue;
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) await discover(file);
    else if (entry.name.endsWith('.jsonl')) { const stat = await fs.stat(file); candidates.push({ file, modified: stat.mtimeMs, bytes: stat.size }); }
  }
}
await discover(path.join(source, 'sessions'));
const selected = candidates.sort((a, b) => b.modified - a.modified).slice(0, Math.max(1, Math.min(10, Number(options.count ?? 6))));
if (!selected.length) throw Error('No supported source files found.');
const hash = text => createHash('sha256').update(text).digest('hex');
function thread(value) {
  if (typeof value !== 'string' || !value) return undefined;
  const h = hash(value); return `${h.slice(0, 8)}-${h.slice(8, 12)}-4${h.slice(13, 16)}-8${h.slice(17, 20)}-${h.slice(20, 32)}`;
}
function identity(value, kind) { return typeof value === 'string' && value ? `${kind}-${hash(value).slice(0, 24)}` : undefined; }
function identifier(value) { return typeof value === 'string' && value.length <= 160 && /^[A-Za-z0-9._:/-]+$/.test(value) ? value : undefined; }
function timeValue(value) { return typeof value === 'string' && value.length <= 50 && Number.isFinite(Date.parse(value)) ? value : Number.isSafeInteger(value) && value >= 0 ? value : undefined; }
function durationValue(value) { return Number.isSafeInteger(value) && value >= 0 ? value : undefined; }
const nativeFields = ['input_tokens', 'output_tokens', 'cached_input_tokens', 'cache_write_input_tokens', 'reasoning_output_tokens', 'total_tokens'];
const facts = new Map();
const report = { selectedFiles: selected.length, sourceBytes: 0, completeLines: 0, malformedLines: 0, oversizedLines: 0, incompleteTails: 0, retainedRecords: 0, sourceDigests: [], expected: null };
function redact(value) {
  const p = value.payload;
  if (!p || typeof p !== 'object' || typeof value.timestamp !== 'string' || !Number.isFinite(Date.parse(value.timestamp))) return null;
  const base = { timestamp: value.timestamp, type: value.type, ...(Number.isSafeInteger(value.ordinal) ? { ordinal: value.ordinal } : {}) };
  const ids = { turn_id: identity(p.turn_id, 'turn'), root_turn_id: identity(p.root_turn_id, 'turn'), parent_thread_id: thread(p.parent_thread_id) };
  if (value.type === 'session_meta') {
    return { ...base, payload: { id: thread(p.id), cli_version: identifier(p.cli_version), cwd: typeof p.cwd === 'string' && p.cwd ? `D:/redacted/project-${hash(p.cwd).slice(0, 8)}` : undefined, parent_thread_id: thread(p.parent_thread_id), subagent_history_start_ordinal: durationValue(p.subagent_history_start_ordinal), source: ['cli','vscode','exec','app_server','appServer','unknown'].includes(p.source) ? p.source : undefined } };
  }
  if (value.type === 'turn_context') return { ...base, payload: { ...ids, model: identifier(p.model), model_provider: identifier(p.model_provider) } };
  if (value.type === 'token_usage_record' && p.usage && typeof p.thread_id === 'string' && p.thread_id && typeof p.response_id === 'string' && p.response_id) {
    const usage = Object.fromEntries(nativeFields.filter(field => p.usage[field] === null || Number.isSafeInteger(p.usage[field])).map(field => [field, p.usage[field]]));
    return { ...base, payload: { ...ids, thread_id: thread(p.thread_id), response_id: identity(p.response_id, 'response'), model: identifier(p.model), usage } };
  }
  if (value.type === 'event_msg' && ['task_started', 'task_complete', 'turn_aborted', 'task_failed'].includes(p.type)) {
    return { ...base, payload: { ...ids, type: p.type, started_at: timeValue(p.started_at), completed_at: timeValue(p.completed_at), duration_ms: durationValue(p.duration_ms), time_to_first_token_ms: durationValue(p.time_to_first_token_ms), status: ['completed','failed','interrupted','unknown','in_progress'].includes(p.status) ? p.status : undefined } };
  }
  return null;
}
function count(record) {
  if (record.type !== 'token_usage_record') return;
  const p = record.payload, usage = p.usage, key = `${p.thread_id}|${p.response_id}`;
  const signature = JSON.stringify(nativeFields.map(field => usage[field] ?? null));
  const previous = facts.get(key);
  if (previous) { if (previous.signature !== signature) previous.conflict = true; return; }
  facts.set(key, { signature, usage, conflict: false });
}
for (let index = 0; index < selected.length; index++) {
  const file = selected[index];
  const digest = createHash('sha256');
  const output = [];
  let fragments = [], size = 0, tooLarge = false;
  function consume(bytes) {
    report.completeLines++;
    if (tooLarge) report.oversizedLines++;
    else {
      try { const clean = redact(JSON.parse(bytes.toString('utf8'))); if (clean) { output.push(JSON.stringify(clean)); count(clean); report.retainedRecords++; } }
      catch { report.malformedLines++; }
    }
    fragments = []; size = 0; tooLarge = false;
  }
  if (file.bytes > 0) for await (const chunk of createReadStream(file.file, { start: 0, end: file.bytes - 1, highWaterMark: 64 * 1024 })) {
    digest.update(chunk); report.sourceBytes += chunk.length;
    let start = 0;
    while (start < chunk.length) {
      const newline = chunk.indexOf(10, start), end = newline < 0 ? chunk.length : newline;
      const slice = chunk.subarray(start, end);
      size += slice.length;
      if (size > 4 * 1024 * 1024) { fragments = []; tooLarge = true; }
      else if (!tooLarge) fragments.push(slice);
      if (newline >= 0) consume(tooLarge ? Buffer.alloc(0) : Buffer.concat(fragments, size));
      start = newline < 0 ? chunk.length : newline + 1;
    }
  }
  if (size > 0 || tooLarge) report.incompleteTails++;
  report.sourceDigests.push(digest.digest('hex'));
  await fs.writeFile(path.join(target, `sample-${index + 1}.jsonl`), output.join('\n') + (output.length ? '\n' : ''));
}
const sums = Object.fromEntries(nativeFields.map(field => [field, 0]));
let accepted = 0, conflicts = 0, invalid = 0;
for (const fact of facts.values()) {
  if (fact.conflict) { conflicts++; continue; }
  const u = fact.usage;
  if (![u.input_tokens, u.output_tokens, u.total_tokens].every(n => Number.isSafeInteger(n) && n >= 0) || u.input_tokens + u.output_tokens !== u.total_tokens) { invalid++; continue; }
  accepted++;
  for (const field of nativeFields) if (Number.isSafeInteger(u[field]) && u[field] >= 0) sums[field] += u[field];
}
report.expected = { uniqueNativeIdentities: facts.size, meteredResponses: accepted, conflicts, invalid, sums, totalRule: 'input + output; no caches or reasoning added twice', scope: 'native top-level response usage only; no aggregate/legacy/tools/desktop qualification' };
await fs.writeFile(path.join(target, 'oracle.json'), JSON.stringify({ createdAt: new Date().toISOString(), ...report }, null, 2) + '\n');
process.stdout.write(JSON.stringify({ outputDirectory: target, ...report }, null, 2) + '\n');
