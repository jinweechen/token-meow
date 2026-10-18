import type { UsagePriceSyncConfig } from "./contracts";
import type { CatalogEntry } from "./usagePriceCatalog";

export const PRICE_SYNC_INTERVAL_MS = 6 * 60 * 60 * 1000;
export const PRICE_SYNC_RETRY_MS = 15 * 60 * 1000;

export function priceSyncDue(
  config: UsagePriceSyncConfig,
  now: number,
  localAttempt: number | null,
): boolean {
  if (!config.enabled || !config.mappings.length || !Number.isFinite(now))
    return false;
  const recent = (value: string | null, interval: number) => {
    const time = value == null ? NaN : Date.parse(value);
    return Number.isFinite(time) && time <= now && now - time < interval;
  };
  if (
    localAttempt != null &&
    localAttempt <= now &&
    now - localAttempt < PRICE_SYNC_RETRY_MS
  )
    return false;
  return (
    !recent(config.lastSuccessAt, PRICE_SYNC_INTERVAL_MS) &&
    !recent(config.lastAttemptAt, PRICE_SYNC_RETRY_MS)
  );
}

/** Resolve the saved source identity exactly; never normalize across suppliers. */
export function priceSyncItems(
  config: UsagePriceSyncConfig,
  entries: CatalogEntry[],
) {
  const catalog = new Map(
    entries.map((entry) => [
      JSON.stringify([entry.provider, entry.model]),
      entry,
    ]),
  );
  const items = config.mappings.flatMap((mapping) => {
    const entry = catalog.get(
      JSON.stringify([mapping.catalogProvider, mapping.catalogModel]),
    );
    if (
      !entry?.syncable ||
      entry.inputPerMillion == null ||
      entry.outputPerMillion == null
    )
      return [];
    return [
      {
        catalogProvider: mapping.catalogProvider,
        catalogModel: mapping.catalogModel,
        inputPerMillion: entry.inputPerMillion,
        outputPerMillion: entry.outputPerMillion,
        cacheReadPerMillion: entry.cacheReadPerMillion,
        cacheWritePerMillion: entry.cacheWritePerMillion,
      },
    ];
  });
  return {
    revision: config.revision,
    items,
    skipped: config.mappings.length - items.length,
  };
}
