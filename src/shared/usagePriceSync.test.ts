import assert from "node:assert/strict";
import test from "node:test";
import type { UsagePriceSyncConfig } from "./contracts";
import type { CatalogEntry } from "./usagePriceCatalog";
import {
  priceSyncDue,
  priceSyncItems,
  PRICE_SYNC_INTERVAL_MS,
  PRICE_SYNC_RETRY_MS,
} from "./usagePriceSync";

const now = Date.parse("2026-10-16T10:00:00Z");
const config: UsagePriceSyncConfig = {
  schemaVersion: 1,
  revision: 4,
  enabled: true,
  overwriteManual: false,
  mappings: [
    {
      catalogProvider: "openai",
      catalogModel: "gpt-test",
      model: "local-alias",
      providerId: null,
    },
  ],
  lastAttemptAt: null,
  lastSuccessAt: null,
  error: null,
  changed: 0,
  skipped: 0,
};
test("automatic pricing sync respects opt-in, six-hour freshness, failure retry and clock rollback", () => {
  assert.equal(priceSyncDue(config, now, null), true);
  assert.equal(priceSyncDue({ ...config, enabled: false }, now, null), false);
  assert.equal(priceSyncDue({ ...config, mappings: [] }, now, null), false);
  assert.equal(
    priceSyncDue(
      { ...config, lastSuccessAt: new Date(now - 1000).toISOString() },
      now,
      null,
    ),
    false,
  );
  assert.equal(
    priceSyncDue(
      {
        ...config,
        lastSuccessAt: new Date(now - PRICE_SYNC_INTERVAL_MS).toISOString(),
      },
      now,
      null,
    ),
    true,
  );
  assert.equal(
    priceSyncDue(
      {
        ...config,
        lastAttemptAt: new Date(now - 1000).toISOString(),
        error: "offline",
      },
      now,
      null,
    ),
    false,
  );
  assert.equal(priceSyncDue(config, now, now - PRICE_SYNC_RETRY_MS + 1), false);
  assert.equal(
    priceSyncDue(
      { ...config, lastSuccessAt: new Date(now + 1000).toISOString() },
      now,
      null,
    ),
    true,
  );
});
test("sync uses exact selected catalog identity, preserves missing cache prices, and reports omitted entries", () => {
  const entry: CatalogEntry = {
    provider: "openai",
    providerName: "OpenAI",
    model: "gpt-test",
    name: "test",
    inputPerMillion: "2",
    outputPerMillion: "10",
    cacheReadPerMillion: "0.2",
    cacheWritePerMillion: null,
    importable: false,
    syncable: true,
    syncReason: null,
    reason: "missing cache write",
  };
  const other = { ...entry, provider: "reseller", inputPerMillion: "500" };
  const payload = priceSyncItems(config, [other, entry]);
  assert.equal(payload.items[0].inputPerMillion, "2");
  assert.equal(payload.items[0].cacheWritePerMillion, null);
  assert.equal(payload.skipped, 0);
  assert.deepEqual(priceSyncItems(config, [other]), {
    revision: 4,
    items: [],
    skipped: 1,
  });
  assert.equal(
    priceSyncItems(config, [{ ...entry, syncable: false }]).skipped,
    1,
  );
});
