import assert from "node:assert/strict";
import test from "node:test";
import {
  catalogPriceDecimal,
  fetchModelsDevCatalog,
  MODELS_DEV_CATALOG_URL,
  parseModelsDevCatalog,
} from "./usagePriceCatalog";

function fixture(cost: Record<string, unknown> = {
  input: 1.25, output: 10, cache_read: 0.125, cache_write: 1.5,
}): Record<string, unknown> {
  return {
    "provider-a": {
      id: "provider-a", name: "Provider A",
      models: { "api-model": { id: "api-model", name: "Display Model", cost } },
    },
  };
}

test("catalog normalizes numeric prices to exact six-place strings without inventing zero", () => {
  for (const [value, expected] of [
    [0, "0.000000"], [1.25, "1.250000"], [1e-6, "0.000001"],
    [0.000011, "0.000011"], [1_000_000, "1000000.000000"],
  ] as const) assert.equal(catalogPriceDecimal(value), expected);
  for (const value of [1e-7, 0.1234567, -1, Infinity, NaN, 1_000_000.000001, "1.25", null, undefined]) {
    assert.equal(catalogPriceDecimal(value), null);
  }
  const [entry] = parseModelsDevCatalog(fixture());
  assert.equal(entry.model, "api-model");
  assert.equal(entry.provider, "provider-a");
  assert.equal(entry.name, "Display Model");
  assert.equal(entry.inputPerMillion, "1.250000");
  assert.equal(entry.outputPerMillion, "10.000000");
  assert.equal(entry.importable, true);
  assert.equal(entry.reason, null);
  assert.equal(entry.syncable, true);
  assert.equal(entry.syncReason, null);
});

test("missing cache prices remain null and make a catalog row ineligible for direct import", () => {
  const [entry] = parseModelsDevCatalog(fixture({ input: 1, output: 2 }));
  assert.equal(entry.cacheReadPerMillion, null);
  assert.equal(entry.cacheWritePerMillion, null);
  assert.equal(entry.importable, false);
  assert.equal(entry.syncable, true);
  assert.equal(entry.syncReason, null);
  assert.match(entry.reason!, /cache_read/u);
  assert.match(entry.reason!, /cache_write/u);
  const [zero] = parseModelsDevCatalog(fixture({ input: 0, output: 0, cache_read: 0, cache_write: 0 }));
  assert.equal(zero.importable, true);
  assert.equal(zero.cacheReadPerMillion, "0.000000");
});

test("bad prices and finer precision cannot silently become rounded or zero prices", () => {
  for (const price of [-1, NaN, Infinity, "1", 0.0000001, 1_000_001]) {
    const [entry] = parseModelsDevCatalog(fixture({ input: price, output: 2, cache_read: 0, cache_write: 0 }));
    assert.equal(entry.inputPerMillion, null);
    assert.equal(entry.importable, false);
    assert.equal(entry.syncable, false);
    assert.match(entry.reason!, /价格无效/u);
  }
});

test("same model names across providers stay separate and canonical IDs never overwrite API IDs", () => {
  const data = fixture();
  data["provider-b"] = {
    id: "provider-b", name: "Provider B",
    models: {
      "api-model": {
        id: "api-model", name: "Display Model", canonical_model_id: "lab/canonical-model",
        cost: { input: 20, output: 40, cache_read: 2, cache_write: 25 },
      }
    },
  };
  const entries = parseModelsDevCatalog(data);
  assert.equal(entries.length, 2);
  assert.deepEqual(entries.map(entry => entry.provider), ["provider-a", "provider-b"]);
  assert.deepEqual(entries.map(entry => entry.model), ["api-model", "api-model"]);
  assert.deepEqual(entries.map(entry => entry.inputPerMillion), ["1.250000", "20.000000"]);
  assert.ok(entries.every(entry => !("providerId" in entry)));
});

test("tiered prices and distinct reasoning rates require manual review", () => {
  for (const extra of [
    { tiers: [{ tier: { type: "context", size: 200_000 }, input: 2, output: 10 }] },
    { context_over_200k: { input: 2, output: 10 } },
    { reasoning: 20 }, { future_fee: 1 }, { input_audio: 1 }, { output_audio: 1 },
  ]) {
    const [entry] = parseModelsDevCatalog(fixture({ input: 1, output: 10, cache_read: 0, cache_write: 0, ...extra }));
    assert.equal(entry.importable, false);
    assert.equal(entry.syncable, false);
    assert.ok(entry.reason);
    assert.equal(entry.inputPerMillion, "1.000000");
  }
  const [sameReasoning] = parseModelsDevCatalog(fixture({ input: 1, output: 10, cache_read: 0, cache_write: 0, reasoning: 10 }));
  assert.equal(sameReasoning.importable, true);
});

test("unknown or malformed catalog envelopes and identities fail without guessing", () => {
  for (const invalid of [null, [], "text", { provider: [] }, { provider: { models: [] } }]) {
    assert.throws(() => parseModelsDevCatalog(invalid), /models.dev/u);
  }
  assert.throws(() => parseModelsDevCatalog({ a: { id: "b", models: {} } }), /身份/u);
  assert.throws(() => parseModelsDevCatalog({ a: { models: { "": { cost: {} } } } }), /身份/u);
  assert.throws(() => parseModelsDevCatalog({ a: { models: { ["x".repeat(161)]: {} } } }), /身份/u);
  const data = fixture();
  const provider = data["provider-a"] as { models: Record<string, { id: string; }>; };
  provider.models["api-model"].id = "different-api-id";
  const [entry] = parseModelsDevCatalog(data);
  assert.equal(entry.model, "different-api-id");
  assert.equal(entry.importable, false);
  assert.equal(entry.syncable, false);
  assert.match(entry.reason!, /目录键不一致/u);
});

test("sync eligibility requires input and output while unknown cache rates remain null", () => {
  const [cacheUnknown] = parseModelsDevCatalog(fixture({ input: 1, output: 2 }));
  assert.equal(cacheUnknown.syncable, true);
  assert.equal(cacheUnknown.cacheReadPerMillion, null);
  assert.equal(cacheUnknown.cacheWritePerMillion, null);
  for (const cost of [{ input: 1 }, { output: 2 }, {}]) {
    const [entry] = parseModelsDevCatalog(fixture(cost));
    assert.equal(entry.syncable, false);
    assert.match(entry.syncReason!, /输入或输出/u);
  }
});

test("catalog fetch is explicit, fixed to official source and sends no credentials", async t => {
  let request: { input: RequestInfo | URL; init?: RequestInit; } | undefined;
  t.mock.method(globalThis, "fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
    request = { input, init };
    return new Response(JSON.stringify(fixture()), { status: 200 });
  });
  const entries = await fetchModelsDevCatalog();
  assert.equal(entries.length, 1);
  assert.equal(request!.input, MODELS_DEV_CATALOG_URL);
  assert.equal(request!.init?.credentials, "omit");
  assert.equal(request!.init?.redirect, "error");
  assert.equal(request!.init?.cache, "no-store");
  assert.deepEqual(request!.init?.headers, { Accept: "application/json" });
  assert.ok(request!.init?.signal instanceof AbortSignal);
});

test("HTTP failures and malformed JSON reject instead of returning an empty catalog", async t => {
  t.mock.method(globalThis, "fetch", async () => new Response("offline", { status: 503 }));
  await assert.rejects(fetchModelsDevCatalog(), /HTTP 503/u);
  t.mock.method(globalThis, "fetch", async () => new Response("<html>not json</html>"));
  await assert.rejects(fetchModelsDevCatalog());
});

test("oversized catalogs are bounded by declared and streamed byte counts", async t => {
  t.mock.method(globalThis, "fetch", async () => new Response("{}", {
    headers: { "content-length": String(16 * 1024 * 1024 + 1) },
  }));
  await assert.rejects(fetchModelsDevCatalog(), /16MiB/u);
  let cancelled = false;
  t.mock.method(globalThis, "fetch", async () => new Response(new ReadableStream<Uint8Array>({
    start(controller) { controller.enqueue(new Uint8Array(16 * 1024 * 1024 + 1)); },
    cancel() { cancelled = true; },
  })));
  await assert.rejects(fetchModelsDevCatalog(), /16MiB/u);
  assert.equal(cancelled, true);
});

test("fetch timeout is bounded and requests manual retry", async t => {
  const realSetTimeout = globalThis.setTimeout;
  t.mock.method(globalThis, "setTimeout", ((callback: () => void) => realSetTimeout(callback, 0)) as typeof setTimeout);
  t.mock.method(globalThis, "fetch", (_input: RequestInfo | URL, init?: RequestInit) => new Promise<Response>((_resolve, reject) => {
    init?.signal?.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")), { once: true });
  }));
  await assert.rejects(fetchModelsDevCatalog(), /超时.*手动重试/u);
});
