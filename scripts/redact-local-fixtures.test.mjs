import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';

test('redaction preserves counts and duplicate identity while excluding source content and structured metadata injections', async () => {
  const temporary = await fs.mkdtemp(path.join(os.tmpdir(), 'token-meow-redactor-'));
  const source = path.join(temporary, 'source'), output = path.join(temporary, 'redacted');
  try {
    await fs.mkdir(path.join(source, 'sessions'), { recursive: true });
    const stamp = new Date().toISOString();
    const usage = { input_tokens: 50, output_tokens: 10, total_tokens: 60, cached_input_tokens: 20 };
    const native = { timestamp: stamp, type: 'token_usage_record', payload: { thread_id: 'private-thread-id', response_id: 'private-response-id', model: { prompt: 'SECRET structured injection' }, usage, transcript: 'SECRET body' } };
    const records = [
      { timestamp: stamp, type: 'session_meta', payload: { id: 'private-thread-id', cwd: 'D:/SECRET-project', cli_version: '0.147.0', source: 'SECRET free text' } },
      native, native,
      { timestamp: stamp, type: 'response_item', payload: { content: 'SECRET user and tool body' } },
    ];
    const file = path.join(source, 'sessions', 'source.jsonl');
    const original = records.map(value => JSON.stringify(value)).join('\n') + '\n';
    await fs.writeFile(file, original);
    const result = spawnSync(process.execPath, [path.resolve('scripts/redact-local-fixtures.mjs'), `--source=${source}`, `--out=${output}`, '--count=1'], { encoding: 'utf8', timeout: 10000 });
    assert.equal(result.status, 0, result.stderr);
    const fixture = await fs.readFile(path.join(output, 'sample-1.jsonl'), 'utf8');
    assert.equal(fixture.includes('SECRET'), false);
    assert.equal(fixture.includes('private-thread-id'), false);
    assert.equal(fixture.includes('private-response-id'), false);
    const oracle = JSON.parse(await fs.readFile(path.join(output, 'oracle.json'), 'utf8'));
    assert.equal(oracle.expected.meteredResponses, 1);
    assert.equal(oracle.expected.sums.total_tokens, 60);
    assert.equal(oracle.expected.sums.cached_input_tokens, 20);
    assert.equal(await fs.readFile(file, 'utf8'), original);
    assert.equal(oracle.sourceDigests[0], createHash('sha256').update(original).digest('hex'));
  } finally {
    const resolved = await fs.realpath(temporary);
    const expectedParent = await fs.realpath(os.tmpdir());
    assert.equal(path.dirname(resolved), expectedParent);
    assert.ok(path.basename(resolved).startsWith('token-meow-redactor-'));
    await fs.rm(resolved, { recursive: true, force: true });
  }
});
