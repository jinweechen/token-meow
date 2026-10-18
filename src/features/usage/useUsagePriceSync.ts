import { useCallback, useEffect, useRef, useState } from "react";
import type { UsagePriceSyncConfig } from "../../shared/contracts";
import * as bridge from "../../shared/bridge";
import { fetchModelsDevCatalog } from "../../shared/usagePriceCatalog";
import { priceSyncDue, priceSyncItems } from "../../shared/usagePriceSync";

/** The main renderer owns this lightweight sync; panel tabs never start timers. */
export function useUsagePriceSync(
  config: UsagePriceSyncConfig | undefined,
  onChanged: () => Promise<void>,
) {
  const current = useRef({ config, onChanged });
  current.current = { config, onChanged };
  const flight = useRef<Promise<void> | null>(null);
  const localAttempt = useRef<number | null>(null);
  const observedRevision = useRef(config?.revision);
  const [syncing, setSyncing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = useCallback((force = true): Promise<void> => {
    if (flight.current) return flight.current;
    const initial = current.current.config;
    if (!bridge.isDesktop)
      return Promise.reject(new Error("浏览器演示不保存同步配置"));
    if (!initial?.mappings.length)
      return Promise.reject(new Error("请先选择并保存要同步的模型"));
    if (!force && !priceSyncDue(initial, Date.now(), localAttempt.current))
      return Promise.resolve();
    localAttempt.current = Date.now();
    setSyncing(true);
    setError(null);
    const task = async () => {
      try {
        const catalog = await fetchModelsDevCatalog();
        const latest = current.current.config;
        // Config edits and disabling during a fetch revoke the old selection.
        if (
          !latest ||
          latest.revision !== initial.revision ||
          (!force && !latest.enabled)
        )
          return;
        const payload = priceSyncItems(initial, catalog);
        if (!payload.items.length)
          throw new Error(
            "所选模型在目录中缺失或价格暂不支持同步，已保留全部本地价格",
          );
        await bridge.applyUsagePriceSync(payload);
        await current.current.onChanged();
      } catch (failure) {
        const message =
          failure instanceof Error ? failure.message : String(failure);
        setError(message);
        const receiptError = message
          .replace(/[\u0000-\u001f\u007f-\u009f]/g, " ")
          .slice(0, 500);
        try {
          await bridge.recordUsagePriceSyncError({
            revision: initial.revision,
            error: receiptError,
          });
        } catch {
          /* A newer saved configuration must not be overwritten by an old failure. */
        }
        await current.current.onChanged();
        throw failure;
      } finally {
        setSyncing(false);
        flight.current = null;
      }
    };
    // Defer execution until flight ownership is published.
    const result = Promise.resolve().then(task);
    flight.current = result;
    return result;
  }, []);
  useEffect(() => {
    if (!bridge.isDesktop) return;
    const previousRevision = observedRevision.current;
    observedRevision.current = config?.revision;
    const changed =
      previousRevision != null && config?.revision !== previousRevision;
    if (changed) localAttempt.current = null;
    const tick = () => {
      const next = current.current.config;
      if (next && priceSyncDue(next, Date.now(), localAttempt.current))
        void run(false).catch(() => undefined);
    };
    if (changed && config?.enabled && config.mappings.length) {
      const expectedRevision = config.revision;
      const applyChangedSelection = () => {
        const latest = current.current.config;
        if (latest?.revision === expectedRevision && latest.enabled)
          void run(true).catch(() => undefined);
      };
      if (flight.current)
        void flight.current.catch(() => undefined).then(applyChangedSelection);
      else applyChangedSelection();
    } else tick();
    const timer = window.setInterval(tick, 60_000);
    return () => window.clearInterval(timer);
  }, [config?.revision, run]);
  return { syncing, error, syncNow: useCallback(() => run(true), [run]) };
}
