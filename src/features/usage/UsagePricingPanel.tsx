import { useId, useState } from "react";
import type { FormEvent } from "react";
import type {
  UsageDashboardData,
  UsagePriceInput,
  UsagePriceSyncConfig,
  UsagePriceSyncConfigInput,
} from "../../shared/contracts";
import {
  shanghaiDateTimeInput,
  shanghaiEffectiveFrom,
  usageTime,
  validPriceDecimal,
} from "../../shared/usageDashboard";
import type { CatalogEntry } from "../../shared/usagePriceCatalog";
import { UsagePriceCatalog } from "./UsagePriceCatalog";
import { UsagePriceSyncPanel } from "./UsagePriceSyncPanel";

const fields = [
  ["inputPerMillion", "新增输入"],
  ["outputPerMillion", "输出"],
  ["cacheReadPerMillion", "缓存读取"],
  ["cacheWritePerMillion", "缓存写入"],
] as const;

export function UsagePricingPanel({
  data,
  onSavePrice,
  isDemo,
  priceSyncConfig,
  onSavePriceSyncConfig,
  onSyncPricesNow,
  priceSyncing,
}: {
  data: UsageDashboardData;
  onSavePrice: (payload: UsagePriceInput) => Promise<void>;
  isDemo: boolean;
  priceSyncConfig?: UsagePriceSyncConfig;
  onSavePriceSyncConfig: (input: UsagePriceSyncConfigInput) => Promise<void>;
  onSyncPricesNow: () => Promise<void>;
  priceSyncing: boolean;
}) {
  const id = useId();
  const [form, setForm] = useState({
    model: "",
    providerId: "",
    effectiveFrom: shanghaiDateTimeInput(),
    inputPerMillion: "",
    outputPerMillion: "",
    cacheReadPerMillion: "",
    cacheWritePerMillion: "",
  });
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [manualOpen, setManualOpen] = useState(false);
  const [catalogSource, setCatalogSource] = useState<CatalogEntry | null>(null);
  const [catalogConfirmed, setCatalogConfirmed] = useState(false);
  const update = (key: keyof typeof form, value: string) => {
    setForm((previous) => ({ ...previous, [key]: value }));
    setSaved(false);
    setError(null);
    setCatalogConfirmed(false);
  };
  function chooseCatalog(entry: CatalogEntry) {
    if (pending) return;
    const manualRatesRequired =
      !entry.importable && fields.every(([key]) => entry[key] != null);
    setCatalogSource(entry);
    setCatalogConfirmed(false);
    setManualOpen(true);
    setSaved(false);
    setError(null);
    setForm((previous) => ({
      ...previous,
      model: entry.model,
      providerId: "",
      inputPerMillion: manualRatesRequired ? "" : (entry.inputPerMillion ?? ""),
      outputPerMillion: manualRatesRequired
        ? ""
        : (entry.outputPerMillion ?? ""),
      cacheReadPerMillion: manualRatesRequired
        ? ""
        : (entry.cacheReadPerMillion ?? ""),
      cacheWritePerMillion: manualRatesRequired
        ? ""
        : (entry.cacheWritePerMillion ?? ""),
    }));
  }
  async function submit(event: FormEvent) {
    event.preventDefault();
    const effectiveFrom = shanghaiEffectiveFrom(form.effectiveFrom);
    if (
      !form.model.trim() ||
      new TextEncoder().encode(form.model.trim()).length > 160 ||
      form.model.trim().toLowerCase() === "unknown"
    ) {
      setError(
        "请输入明确的模型名称（最多 160 字节）。未知模型不能自动匹配价格。",
      );
      return;
    }
    if (!effectiveFrom) {
      setError("请填写有效的上海时区生效时间。");
      return;
    }
    if (fields.some(([key]) => !validPriceDecimal(form[key]))) {
      setError(
        "四类价格都需要填写 0 到 1,000,000 之间的非负十进制数，最多 6 位小数，单位 USD / 百万 Token。",
      );
      return;
    }
    if (catalogSource && !catalogConfirmed) {
      setError("请先确认目录价格与目标本地模型、供应商、生效时间一致。");
      return;
    }
    setPending(true);
    setError(null);
    setSaved(false);
    try {
      await onSavePrice({
        model: form.model.trim(),
        providerId: form.providerId.trim() || null,
        effectiveFrom,
        inputPerMillion: form.inputPerMillion.trim(),
        outputPerMillion: form.outputPerMillion.trim(),
        cacheReadPerMillion: form.cacheReadPerMillion.trim(),
        cacheWritePerMillion: form.cacheWritePerMillion.trim(),
      });
      setSaved(true);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setPending(false);
    }
  }
  return (
    <div className="usage-pricing-panel">
      <div className="usage-pricing-intro">
        <h3>模型价格版本</h3>
        <p>
          按模型、供应商和响应发生时间匹配价格。所有金额为本地估算，与账户实际扣费分别记录。
        </p>
      </div>
      <UsagePriceSyncPanel
        config={priceSyncConfig}
        onSaveConfig={onSavePriceSyncConfig}
        onSyncNow={onSyncPricesNow}
        syncing={priceSyncing}
        isDemo={isDemo}
      />
      <UsagePriceCatalog onChoose={chooseCatalog} saving={pending} />
      <details
        className="usage-manual-price"
        open={manualOpen}
        onToggle={(event) => setManualOpen(event.currentTarget.open)}
      >
        <summary>
          新增本地价格版本 <span aria-hidden="true">＋</span>
        </summary>
        <form className="usage-price-form" onSubmit={submit}>
          {catalogSource && (
            <div className="usage-catalog-selection">
              <strong>正在核对目录价格</strong>
              <span>
                源模型：{catalogSource.model} · 源供应商：
                {catalogSource.providerName}（{catalogSource.provider}）
              </span>
              <span>
                源供应商不会自动绑定为本地供应商。请确认下方目标模型及适用范围。
              </span>
              {!catalogSource.importable && (
                <span className="usage-catalog-warning">
                  {catalogSource.reason ?? "目录价格不完整或不可直接使用"}
                  。请核对来源并人工补齐本地估算价格；无法表达的分段或独立计费不会自动套用。
                </span>
              )}
              <button
                type="button"
                className="usage-catalog-clear"
                onClick={() => {
                  setCatalogSource(null);
                  setCatalogConfirmed(false);
                }}
              >
                改为手动填写
              </button>
            </div>
          )}
          <div className="usage-price-identity">
            <label>
              目标本地模型 <span>必填</span>
              <input
                required
                list={`${id}-models`}
                value={form.model}
                disabled={pending}
                onChange={(event) => update("model", event.target.value)}
                placeholder="例如 gpt-6.1-sol"
                autoComplete="off"
              />
              <datalist id={`${id}-models`}>
                {data.models
                  .filter(
                    (model) =>
                      model.name !== "未知" && model.name !== "unknown",
                  )
                  .map((model) => (
                    <option key={model.key} value={model.name} />
                  ))}
              </datalist>
            </label>
            <label>
              目标本地供应商 <span>留空适用于所有供应商</span>
              <input
                list={`${id}-providers`}
                value={form.providerId}
                disabled={pending}
                onChange={(event) => update("providerId", event.target.value)}
                placeholder="全部供应商"
                autoComplete="off"
              />
              <datalist id={`${id}-providers`}>
                {data.providers
                  .filter(
                    (provider) => provider.key && provider.name !== "未知",
                  )
                  .map((provider) => (
                    <option key={provider.key} value={provider.key}>
                      {provider.name}
                    </option>
                  ))}
              </datalist>
            </label>
            <label>
              生效时间 <span>Asia/Shanghai</span>
              <input
                type="datetime-local"
                required
                value={form.effectiveFrom}
                disabled={pending}
                onChange={(event) =>
                  update("effectiveFrom", event.target.value)
                }
              />
            </label>
          </div>
          <div className="usage-price-fields">
            {fields.map(([key, label]) => (
              <label key={key}>
                {label} <span>USD / M</span>
                <input
                  type="text"
                  inputMode="decimal"
                  required
                  value={form[key]}
                  disabled={pending}
                  onChange={(event) => update(key, event.target.value)}
                  placeholder="0.00"
                  autoComplete="off"
                />
              </label>
            ))}
          </div>
          {catalogSource && (
            <label className="usage-catalog-confirm">
              <input
                type="checkbox"
                checked={catalogConfirmed}
                onChange={(event) => setCatalogConfirmed(event.target.checked)}
                disabled={pending}
              />
              <span>
                我已核对四类价格，并确认目标本地模型、供应商适用范围和上海时区生效时间。
              </span>
            </label>
          )}
          <div className="usage-price-save">
            <span>
              保存会新增版本，保留既有价格历史。
              {isDemo && "演示模式不保存配置；下方价格仅为示例。"}
            </span>
            <button
              className="usage-primary"
              type="submit"
              disabled={
                pending || isDemo || (!!catalogSource && !catalogConfirmed)
              }
            >
              {pending
                ? "正在保存…"
                : isDemo
                  ? "演示模式不可保存"
                  : "保存新价格版本"}
            </button>
          </div>
          {error && (
            <p className="usage-form-error" role="alert">
              {error}
            </p>
          )}
          {saved && (
            <p className="usage-form-success" role="status">
              新价格版本已保存。用量估算会按生效时间重新匹配。
            </p>
          )}
        </form>
      </details>
      <div className="usage-version-heading">
        <h3>历史版本</h3>
        <span>{data.pricingVersions.length} 个版本 · USD / 百万 Token</span>
      </div>
      <div className="usage-table-scroll">
        <table className="usage-table usage-price-table">
          <caption className="usage-sr-only">
            模型价格版本历史，单位美元每百万 Token
          </caption>
          <thead>
            <tr>
              <th>版本 / 模型</th>
              <th>供应商</th>
              <th>生效时间（上海）</th>
              {fields.map(([key, label]) => (
                <th key={key} className="number">
                  {label}
                </th>
              ))}
              <th>创建时间</th>
            </tr>
          </thead>
          <tbody>
            {data.pricingVersions.map((price) => (
              <tr key={price.id}>
                <td>
                  <strong>{price.model}</strong>
                  <small>
                    v{price.revision} · {price.id}
                  </small>
                  {price.origin && (
                    <small
                      title={`目录模型 ${price.origin.catalogModel} · 同步于 ${usageTime(price.origin.syncedAt, true)}`}
                    >
                      models.dev · {price.origin.catalogProvider} /{" "}
                      {price.origin.catalogModel}
                    </small>
                  )}
                </td>
                <td>{price.providerId ?? "全部供应商"}</td>
                <td>{usageTime(price.effectiveFrom, true)}</td>
                {fields.map(([key]) => (
                  <td key={key} className="number">
                    {price[key] ?? "未提供"}
                  </td>
                ))}
                <td>{usageTime(price.createdAt, true)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {!data.pricingVersions.length && (
        <div className="usage-empty">
          <strong>还没有配置价格</strong>
          <span>
            填写模型价格后才能计算估算费用；未定价的记录会保留 Token 原值。
          </span>
        </div>
      )}
    </div>
  );
}
