import { useState } from "react";
import type { CatalogEntry } from "../../shared/usagePriceCatalog";
import { fetchModelsDevCatalog } from "../../shared/usagePriceCatalog";
import { usageTime } from "../../shared/usageDashboard";

export function UsagePriceCatalog({
  onChoose,
  saving,
}: {
  onChoose: (entry: CatalogEntry) => void;
  saving: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [entries, setEntries] = useState<CatalogEntry[] | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [fetchedAt, setFetchedAt] = useState<string | null>(null);
  const [provider, setProvider] = useState("");
  const [model, setModel] = useState("");
  async function load() {
    setPending(true);
    setError(null);
    try {
      setEntries(await fetchModelsDevCatalog());
      setFetchedAt(new Date().toISOString());
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setPending(false);
    }
  }
  const providers = [
    ...new Map(
      (entries ?? []).map((entry) => [entry.provider, entry.providerName]),
    ).entries(),
  ].sort((a, b) => a[1].localeCompare(b[1]));
  const needle = model.trim().toLowerCase();
  const filtered = (entries ?? []).filter(
    (entry) =>
      (!provider || entry.provider === provider) &&
      (!needle ||
        `${entry.model} ${entry.name}`.toLowerCase().includes(needle)),
  );
  const visible = filtered.slice(0, 60);
  return (
    <div className="usage-catalog">
      <button
        type="button"
        className="usage-secondary usage-catalog-trigger"
        aria-expanded={open}
        aria-controls="usage-catalog-panel"
        onClick={() => {
          setOpen(!open);
          if (!open && entries == null && !pending) void load();
        }}
      >
        从 models.dev 导入 <span aria-hidden="true">{open ? "⌃" : "↓"}</span>
      </button>
      {open && (
        <div className="usage-catalog-panel" id="usage-catalog-panel">
          <div className="usage-catalog-heading">
            <div>
              <h3>浏览公开价格目录</h3>
              <p>
                来源{" "}
                <a href="https://models.dev" target="_blank" rel="noreferrer">
                  models.dev
                </a>
                {fetchedAt ? ` · 读取于 ${usageTime(fetchedAt, true)}` : ""}
              </p>
            </div>
            <button
              className="usage-secondary"
              type="button"
              disabled={pending}
              onClick={() => void load()}
            >
              {pending ? "正在读取…" : "刷新目录"}
            </button>
          </div>
          <p className="usage-catalog-note">
            目录仅供参考。选择一项后核对本地模型、供应商和生效时间，再保存新版本。不会自动同步或修改账户实际账单；缺失价格不会填成
            0。
          </p>
          {error && (
            <p className="usage-form-error" role="alert">
              目录读取失败：{error}
              {entries && "。下方保留上次读取的目录。"}
            </p>
          )}
          <div className="usage-catalog-filters">
            <label>
              目录供应商
              <select
                value={provider}
                disabled={pending}
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
                value={model}
                onChange={(event) => setModel(event.target.value)}
                placeholder="输入模型名称或 ID"
              />
            </label>
          </div>
          <div className="usage-table-scroll">
            <table className="usage-table usage-catalog-table">
              <caption className="usage-sr-only">
                models.dev 价格目录预览，单位美元每百万
                Token。价格需用户核对后保存。
              </caption>
              <thead>
                <tr>
                  <th>目录模型 / 供应商</th>
                  <th className="number">新增输入</th>
                  <th className="number">输出</th>
                  <th className="number">缓存读取</th>
                  <th className="number">缓存写入</th>
                  <th>操作</th>
                </tr>
              </thead>
              <tbody>
                {visible.map((entry) => (
                  <tr key={`${entry.provider}:${entry.model}`}>
                    <td>
                      <strong>{entry.model}</strong>
                      <small>
                        {entry.providerName} · {entry.provider}
                      </small>
                      {!entry.importable && (
                        <small className="usage-catalog-warning">
                          {entry.reason ?? "价格不完整，需要手动补齐"}
                        </small>
                      )}
                    </td>
                    {(
                      [
                        "inputPerMillion",
                        "outputPerMillion",
                        "cacheReadPerMillion",
                        "cacheWritePerMillion",
                      ] as const
                    ).map((key) => (
                      <td
                        key={key}
                        className={`number ${entry[key] == null ? "usage-unavailable" : ""}`}
                      >
                        {entry[key] ?? "未提供"}
                      </td>
                    ))}
                    <td>
                      <button
                        type="button"
                        className="usage-catalog-use"
                        disabled={saving}
                        onClick={() => onChoose(entry)}
                      >
                        {entry.importable ? "填入表单" : "检查并补齐"}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {entries && !visible.length && (
            <div className="usage-empty">
              <strong>没有匹配的目录模型</strong>
              <span>试试调整供应商或搜索词，也可以手动新增本地价格。</span>
            </div>
          )}
          <p className="usage-catalog-footer">
            {entries
              ? `${filtered.length} 项匹配${filtered.length > visible.length ? `，显示前 ${visible.length} 项，请缩小筛选范围` : ""} · USD / 百万 Token`
              : pending
                ? "正在从 models.dev 获取目录…"
                : "尚未获取价格目录"}
          </p>
        </div>
      )}
    </div>
  );
}
