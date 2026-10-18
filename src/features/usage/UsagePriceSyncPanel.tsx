import { useEffect, useState } from "react";
import type {
  UsagePriceSyncConfig,
  UsagePriceSyncConfigInput,
} from "../../shared/contracts";
import type { CatalogEntry } from "../../shared/usagePriceCatalog";
import { fetchModelsDevCatalog } from "../../shared/usagePriceCatalog";
import { usageTime } from "../../shared/usageDashboard";

type Mapping = UsagePriceSyncConfigInput["mappings"][number];
const mappingKey = (
  mapping: Pick<Mapping, "catalogProvider" | "catalogModel">,
) => `${mapping.catalogProvider}\u0000${mapping.catalogModel}`;

export function UsagePriceSyncPanel({
  config,
  onSaveConfig,
  onSyncNow,
  syncing,
  isDemo,
}: {
  config: UsagePriceSyncConfig | undefined;
  onSaveConfig: (input: UsagePriceSyncConfigInput) => Promise<void>;
  onSyncNow: () => Promise<void>;
  syncing: boolean;
  isDemo: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [catalog, setCatalog] = useState<CatalogEntry[] | null>(null);
  const [catalogLoading, setCatalogLoading] = useState(false);
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const [provider, setProvider] = useState("");
  const [search, setSearch] = useState("");
  const [mappings, setMappings] = useState<Mapping[]>(config?.mappings ?? []);
  const [overwriteManual, setOverwriteManual] = useState(
    config?.overwriteManual ?? false,
  );
  const [action, setAction] = useState<"save" | "sync" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  useEffect(() => {
    setMappings(config?.mappings ?? []);
    setOverwriteManual(config?.overwriteManual ?? false);
  }, [config?.revision, config != null]);
  const busy = action != null || syncing;
  const dirty =
    JSON.stringify(mappings) !== JSON.stringify(config?.mappings ?? []) ||
    overwriteManual !== (config?.overwriteManual ?? false);
  async function loadCatalog() {
    setCatalogLoading(true);
    setCatalogError(null);
    try {
      setCatalog(await fetchModelsDevCatalog());
    } catch (failure) {
      setCatalogError(
        failure instanceof Error ? failure.message : String(failure),
      );
    } finally {
      setCatalogLoading(false);
    }
  }
  function editModels() {
    setOpen(!open);
    if (!open && catalog == null && !catalogLoading) void loadCatalog();
  }
  function selectEntry(entry: CatalogEntry, checked: boolean) {
    setError(null);
    setMessage(null);
    const key = mappingKey({
      catalogProvider: entry.provider,
      catalogModel: entry.model,
    });
    if (!checked) {
      setMappings((previous) =>
        previous.filter((mapping) => mappingKey(mapping) !== key),
      );
      return;
    }
    if (!entry.syncable) return;
    if (mappings.length >= 100) {
      setError("每次最多选择 100 个同步模型，请先移除不需要的映射。");
      return;
    }
    setMappings((previous) => [
      ...previous,
      {
        catalogProvider: entry.provider,
        catalogModel: entry.model,
        model: entry.model,
        providerId: null,
      },
    ]);
  }
  function updateMapping(
    index: number,
    key: "model" | "providerId",
    value: string,
  ) {
    setMappings((previous) =>
      previous.map((mapping, current) =>
        current === index
          ? { ...mapping, [key]: key === "providerId" ? value || null : value }
          : mapping,
      ),
    );
    setMessage(null);
    setError(null);
  }
  async function save(input: UsagePriceSyncConfigInput) {
    if (isDemo || busy) return;
    if (input.enabled && !input.mappings.length) {
      setError("请先选择并保存至少一个模型，再开启自动同步。");
      setOpen(true);
      if (!catalog && !catalogLoading) void loadCatalog();
      return;
    }
    const targets = new Set<string>();
    for (const mapping of input.mappings) {
      if (
        !mapping.model.trim() ||
        mapping.model.trim().toLowerCase() === "unknown" ||
        new TextEncoder().encode(mapping.model.trim()).length > 160
      ) {
        setError("请为每项填写明确的目标本地模型，最多 160 字节。");
        return;
      }
      const target = `${mapping.model.trim()}\u0000${mapping.providerId?.trim() ?? ""}`;
      if (targets.has(target)) {
        setError("目标本地模型与供应商组合重复，请明确区分映射后保存。");
        return;
      }
      targets.add(target);
    }
    setAction("save");
    setError(null);
    setMessage(null);
    try {
      await onSaveConfig({
        ...input,
        mappings: input.mappings.map((mapping) => ({
          ...mapping,
          model: mapping.model.trim(),
          providerId: mapping.providerId?.trim() || null,
        })),
      });
      setMessage("同步设置已保存。");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setAction(null);
    }
  }
  async function syncNow() {
    if (isDemo || busy) return;
    setAction("sync");
    setError(null);
    setMessage(null);
    try {
      await onSyncNow();
      setMessage("本次同步已完成，结果见最近同步记录。");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setAction(null);
    }
  }
  const providers = [
    ...new Map(
      (catalog ?? []).map((entry) => [entry.provider, entry.providerName]),
    ).entries(),
  ].sort((a, b) => a[1].localeCompare(b[1]));
  const needle = search.trim().toLowerCase();
  const filtered = (catalog ?? []).filter(
    (entry) =>
      (!provider || entry.provider === provider) &&
      (!needle ||
        `${entry.model} ${entry.name}`.toLowerCase().includes(needle)),
  );
  const visible = filtered.slice(0, 60);
  const selected = new Set(mappings.map(mappingKey));
  return (
    <section
      className="usage-price-sync"
      aria-labelledby="usage-price-sync-title"
    >
      <div className="usage-sync-heading">
        <div>
          <h3 id="usage-price-sync-title">models.dev 价格同步</h3>
          <p>
            {config?.enabled ? "自动同步已启用" : "自动同步已关闭"} · 已保存{" "}
            {config?.mappings.length ?? 0} 个模型
            {dirty ? " · 选择有未保存更改" : ""}
          </p>
        </div>
        <div className="usage-sync-actions">
          <label
            className="usage-sync-switch"
            title={dirty ? "请先保存模型选择再切换自动同步" : undefined}
          >
            <input
              type="checkbox"
              role="switch"
              aria-label="自动同步模型价格"
              checked={config?.enabled ?? false}
              disabled={isDemo || busy || dirty}
              onChange={(event) =>
                void save({
                  revision: config?.revision ?? 0,
                  enabled: event.target.checked,
                  overwriteManual: config?.overwriteManual ?? false,
                  mappings: config?.mappings ?? [],
                })
              }
            />
            <span>自动同步</span>
          </label>
          <button
            type="button"
            className="usage-secondary"
            disabled={busy}
            onClick={editModels}
          >
            {open ? "收起模型" : "选择模型"}
          </button>
          <button
            type="button"
            className="usage-primary"
            disabled={isDemo || busy || dirty || !config?.mappings.length}
            title={dirty ? "请先保存模型选择再同步" : undefined}
            onClick={() => void syncNow()}
          >
            {syncing || action === "sync" ? "正在同步…" : "立即同步"}
          </button>
        </div>
      </div>
      <div className="usage-sync-status">
        <span>最近成功 {usageTime(config?.lastSuccessAt, true)}</span>
        <span>最近尝试 {usageTime(config?.lastAttemptAt, true)}</span>
        {config?.lastAttemptAt && (
          <span>
            新增价格 {config.changed} 项 · 跳过 {config.skipped} 项
          </span>
        )}
      </div>
      <p className="usage-sync-note">
        只同步明确选择的模型，价格变化新增版本；默认保护手动价格。缺少缓存价格保持“未提供”，不会补为
        0。{isDemo && "浏览器演示不保存同步设置。"}
      </p>
      <p className="usage-sync-note">
        新同步价格从同步时点起生效。此前响应若需估算，可在“新增本地价格版本”中核对并填写较早的生效时间。
      </p>
      {config?.error && (
        <p className="usage-form-error" role="status">
          最近同步提示：{config.error}。现有价格版本继续保留。
        </p>
      )}
      {error && (
        <p className="usage-form-error" role="alert">
          {error}
        </p>
      )}
      {message && (
        <p className="usage-form-success" role="status">
          {message}
        </p>
      )}
      {open && (
        <div className="usage-sync-editor">
          <div className="usage-sync-editor-heading">
            <h4>
              选择同步模型 <span>{mappings.length} / 100</span>
            </h4>
            <button
              className="usage-secondary"
              type="button"
              disabled={catalogLoading || busy}
              onClick={() => void loadCatalog()}
            >
              {catalogLoading ? "正在读取目录…" : "刷新目录"}
            </button>
          </div>
          <p className="usage-catalog-note">
            目录供应商代表价格来源；目标本地供应商留空表示适用于全部本地供应商。请核对模型
            ID 与适用范围，避免将不同目录的同名模型绑定到同一目标。
          </p>
          {catalogError && (
            <p className="usage-form-error" role="alert">
              目录读取失败：{catalogError}。已保存映射不受影响。
            </p>
          )}
          <div className="usage-catalog-filters">
            <label>
              目录供应商
              <select
                value={provider}
                onChange={(event) => setProvider(event.target.value)}
              >
                <option value="">全部目录供应商</option>
                {providers.map(([key, name]) => (
                  <option key={key} value={key}>
                    {name} · {key}
                  </option>
                ))}
              </select>
            </label>
            <label>
              搜索模型
              <input
                type="search"
                value={search}
                onChange={(event) => setSearch(event.target.value)}
                placeholder="模型名称或 API ID"
              />
            </label>
          </div>
          <div className="usage-table-scroll">
            <table className="usage-table usage-sync-catalog-table">
              <caption className="usage-sr-only">
                选择要同步的 models.dev
                模型，缺少输入输出价格或计价不支持的条目不可选择。
              </caption>
              <thead>
                <tr>
                  <th>选择</th>
                  <th>目录模型 / 供应商</th>
                  <th className="number">输入 USD/M</th>
                  <th className="number">输出 USD/M</th>
                  <th>同步支持</th>
                </tr>
              </thead>
              <tbody>
                {visible.map((entry) => {
                  const key = mappingKey({
                    catalogProvider: entry.provider,
                    catalogModel: entry.model,
                  });
                  const checked = selected.has(key);
                  return (
                    <tr key={key}>
                      <td>
                        <input
                          className="usage-sync-check"
                          type="checkbox"
                          aria-label={`同步 ${entry.provider} 的 ${entry.model}`}
                          checked={checked}
                          disabled={
                            busy || isDemo || (!checked && !entry.syncable)
                          }
                          onChange={(event) =>
                            selectEntry(entry, event.target.checked)
                          }
                        />
                      </td>
                      <td>
                        <strong>{entry.model}</strong>
                        <small>
                          {entry.providerName} · {entry.provider}
                        </small>
                      </td>
                      <td className="number">
                        {entry.inputPerMillion ?? "未提供"}
                      </td>
                      <td className="number">
                        {entry.outputPerMillion ?? "未提供"}
                      </td>
                      <td title={entry.syncReason ?? undefined}>
                        {entry.syncable ? "可同步" : "不支持"}
                        {entry.syncReason && <small>{entry.syncReason}</small>}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          <p className="usage-catalog-footer">
            {catalog
              ? `${filtered.length} 项匹配${filtered.length > visible.length ? `，显示前 ${visible.length} 项，请缩小筛选范围` : ""}`
              : catalogLoading
                ? "正在获取目录…"
                : "尚未读取目录"}
          </p>
          <div className="usage-sync-mappings-heading">
            <h4>已选目标映射</h4>
            <span>不会自动套用目录供应商身份</span>
          </div>
          {mappings.length ? (
            <div className="usage-table-scroll">
              <table className="usage-table usage-sync-mapping-table">
                <caption className="usage-sr-only">
                  已选同步模型的本地目标映射
                </caption>
                <thead>
                  <tr>
                    <th>目录来源</th>
                    <th>目标本地模型</th>
                    <th>目标本地供应商</th>
                    <th>操作</th>
                  </tr>
                </thead>
                <tbody>
                  {mappings.map((mapping, index) => (
                    <tr key={mappingKey(mapping)}>
                      <td>
                        <strong>{mapping.catalogModel}</strong>
                        <small>{mapping.catalogProvider}</small>
                      </td>
                      <td>
                        <input
                          aria-label={`${mapping.catalogModel} 的目标本地模型`}
                          value={mapping.model}
                          disabled={busy || isDemo}
                          onChange={(event) =>
                            updateMapping(index, "model", event.target.value)
                          }
                        />
                      </td>
                      <td>
                        <input
                          aria-label={`${mapping.catalogModel} 的目标本地供应商`}
                          value={mapping.providerId ?? ""}
                          placeholder="全部本地供应商"
                          disabled={busy || isDemo}
                          onChange={(event) =>
                            updateMapping(
                              index,
                              "providerId",
                              event.target.value,
                            )
                          }
                        />
                      </td>
                      <td>
                        <button
                          type="button"
                          className="usage-catalog-clear"
                          disabled={busy || isDemo}
                          onClick={() =>
                            setMappings((previous) =>
                              previous.filter(
                                (_, current) => current !== index,
                              ),
                            )
                          }
                          aria-label={`移除 ${mapping.catalogModel} 同步映射`}
                        >
                          移除
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <div className="usage-sync-empty">
              尚未选择同步模型。请在上方目录中勾选需要的条目。
            </div>
          )}
          <label className="usage-sync-overwrite">
            <input
              type="checkbox"
              checked={overwriteManual}
              disabled={busy || isDemo}
              onChange={(event) => {
                setOverwriteManual(event.target.checked);
                setMessage(null);
              }}
            />
            <span>
              允许同步覆盖手动价格
              <small>
                默认关闭；开启后，匹配模型的目录价格变化可新增版本覆盖手动价，原历史保留。
              </small>
            </span>
          </label>
          <div className="usage-sync-save">
            <span>
              {dirty ? "当前选择尚未保存" : "同步设置与已保存配置一致"}
            </span>
            <button
              type="button"
              className="usage-primary"
              disabled={busy || isDemo || !dirty}
              onClick={() =>
                void save({
                  revision: config?.revision ?? 0,
                  enabled: config?.enabled ?? false,
                  overwriteManual,
                  mappings,
                })
              }
            >
              {action === "save" ? "正在保存…" : "保存同步选择"}
            </button>
          </div>
        </div>
      )}
    </section>
  );
}
