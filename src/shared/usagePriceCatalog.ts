/**
 * Read-only models.dev preview. No credentials, local provider mapping, or writes.
 * Source: https://models.dev/api.json (community-maintained, USD / million tokens).
 * Schema: https://github.com/anomalyco/models.dev/blob/dev/packages/core/src/schema.ts
 * Upstream license: https://github.com/anomalyco/models.dev/blob/dev/LICENSE (MIT).
 */
export const MODELS_DEV_CATALOG_URL = "https://models.dev/api.json";
export const MODELS_DEV_SOURCE_URL = "https://github.com/anomalyco/models.dev";
export const MODELS_DEV_LICENSE_URL = `${MODELS_DEV_SOURCE_URL}/blob/dev/LICENSE`;

export interface CatalogEntry {
  /** Provider-specific API model ID; never replace it with canonical_model_id. */
  model: string;
  /** Catalog provenance, not an automatically verified local providerId. */
  provider: string;
  providerName: string;
  name: string;
  inputPerMillion: string | null;
  outputPerMillion: string | null;
  cacheReadPerMillion: string | null;
  cacheWritePerMillion: string | null;
  /** All four base prices are representable. Still requires an explicit preview. */
  importable: boolean;
  reason: string | null;
  /** Supports synchronization; absent cache prices stay unknown until usage. */
  syncable: boolean;
  syncReason: string | null;
}

const MAX_BYTES = 16 * 1024 * 1024;
const MAX_ENTRIES = 50_000;
const MAX_PROVIDERS = 1_000;
const PRICE_FIELDS = [
  ["input", "inputPerMillion"],
  ["output", "outputPerMillion"],
  ["cache_read", "cacheReadPerMillion"],
  ["cache_write", "cacheWritePerMillion"],
] as const;
const KNOWN_COST_FIELDS = new Set([
  ...PRICE_FIELDS.map(([source]) => source),
  "reasoning", "input_audio", "output_audio", "tiers", "context_over_200k",
]);

function object(value: unknown): Record<string, unknown> | null {
  return value != null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function label(value: unknown, maximum = 160): string | null {
  return typeof value === "string" && value.length > 0
    && new TextEncoder().encode(value).length <= maximum
    && value.trim() === value && !/[\u0000-\u001f\u007f-\u009f]/u.test(value)
    ? value : null;
}

/** Convert numeric JSON prices without float multiplication or silent rounding. */
export function catalogPriceDecimal(value: unknown): string | null {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0 || value > 1_000_000) {
    return null;
  }
  const parts = /^(\d+)(?:\.(\d+))?(?:e([+-]?\d+))?$/iu.exec(String(value));
  if (!parts) return null;
  const whole = parts[1];
  const fraction = parts[2] ?? "";
  const exponent = Number(parts[3] ?? "0");
  const digits = whole + fraction;
  const position = whole.length + exponent;
  let expanded: string;
  if (position <= 0) expanded = `0.${"0".repeat(-position)}${digits}`;
  else if (position >= digits.length) expanded = digits + "0".repeat(position - digits.length);
  else expanded = `${digits.slice(0, position)}.${digits.slice(position)}`;
  const [integer, rawFraction = ""] = expanded.split(".");
  const decimals = rawFraction.replace(/0+$/u, "");
  // Never round a positive sub-micro price to zero or alter upstream facts.
  if (decimals.length > 6) return null;
  return `${integer}.${decimals.padEnd(6, "0")}`;
}

export function parseModelsDevCatalog(value: unknown): CatalogEntry[] {
  const providers = object(value);
  if (!providers || Object.keys(providers).length > MAX_PROVIDERS) {
    throw new Error("models.dev 目录格式不兼容或供应商数量超出范围");
  }
  const entries: CatalogEntry[] = [];
  for (const [providerKey, rawProvider] of Object.entries(providers)) {
    const provider = object(rawProvider);
    const providerId = label(providerKey);
    const models = object(provider?.models);
    if (!provider || !providerId || !models || (provider.id != null && provider.id !== providerId)) {
      throw new Error("models.dev 供应商结构或身份不兼容，请保留现有本地价格");
    }
    const providerName = label(provider.name, 320) ?? providerId;
    for (const [modelKey, rawModel] of Object.entries(models)) {
      if (entries.length >= MAX_ENTRIES) throw new Error("models.dev 模型数量超出支持范围");
      const model = object(rawModel);
      const modelId = label(model?.id ?? modelKey);
      if (!model || !modelId || modelId === "unknown") {
        throw new Error("models.dev 模型身份不兼容，请保留现有本地价格");
      }
      const cost = object(model.cost);
      const entry: CatalogEntry = {
        model: modelId, provider: providerId, providerName,
        name: label(model.name, 320) ?? modelId,
        inputPerMillion: null, outputPerMillion: null,
        cacheReadPerMillion: null, cacheWritePerMillion: null,
        importable: false, reason: null, syncable: false, syncReason: null,
      };
      const missing: string[] = [];
      const invalid: string[] = [];
      for (const [source, target] of PRICE_FIELDS) {
        const raw = cost?.[source];
        entry[target] = catalogPriceDecimal(raw);
        if (raw == null) missing.push(source);
        else if (entry[target] == null) invalid.push(source);
      }
      const reasons: string[] = [];
      const syncReasons: string[] = [];
      const block = (reason: string) => { reasons.push(reason); syncReasons.push(reason); };
      if (model.id != null && model.id !== modelKey) block("模型ID与目录键不一致，须核对API模型标识");
      if (missing.length) reasons.push(`缺少价格：${missing.join("、")}，请人工补齐`);
      if (missing.some(field => field === "input" || field === "output")) syncReasons.push("缺少输入或输出价格");
      if (invalid.length) block(`价格无效或超出6位精度：${invalid.join("、")}`);
      if (cost?.tiers != null || cost?.context_over_200k != null) {
        block("含分段上下文定价，四项基础价无法完整表达，请人工核对");
      }
      if (cost && Object.keys(cost).some(key => !KNOWN_COST_FIELDS.has(key))) {
        block("含尚未支持的计费字段，请人工核对");
      }
      if (cost?.reasoning != null && cost.reasoning !== cost.output) {
        block("推理Token有独立价格，当前四项定价无法完整表达");
      }
      if (cost?.input_audio != null || cost?.output_audio != null) {
        block("音频Token有独立价格，当前四项定价无法完整表达");
      }
      entry.importable = reasons.length === 0;
      entry.reason = reasons.length ? reasons.join("；") : null;
      entry.syncable = syncReasons.length === 0;
      entry.syncReason = syncReasons.length ? syncReasons.join("；") : null;
      entries.push(entry);
    }
  }
  return entries.sort((left, right) =>
    left.provider.localeCompare(right.provider) || left.model.localeCompare(right.model));
}

async function boundedText(response: Response): Promise<string> {
  const length = response.headers.get("content-length");
  if (length != null && Number(length) > MAX_BYTES) {
    await response.body?.cancel();
    throw new Error("models.dev 目录超过16MiB限制");
  }
  if (!response.body) throw new Error("models.dev 未返回目录内容");
  const reader = response.body.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let bytes = 0;
  let text = "";
  try {
    while (true) {
      const chunk = await reader.read();
      if (chunk.done) break;
      bytes += chunk.value.byteLength;
      if (bytes > MAX_BYTES) throw new Error("models.dev 目录超过16MiB限制");
      text += decoder.decode(chunk.value, { stream: true });
    }
    return text + decoder.decode();
  } catch (error) {
    await reader.cancel().catch(() => undefined);
    throw error;
  } finally {
    reader.releaseLock();
  }
}

/** Read-only transport; scheduling, mapping, and saving belong to the caller. */
export async function fetchModelsDevCatalog(): Promise<CatalogEntry[]> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 15_000);
  try {
    const response = await fetch(MODELS_DEV_CATALOG_URL, {
      method: "GET", credentials: "omit", cache: "no-store", redirect: "error",
      signal: controller.signal, headers: { Accept: "application/json" },
    });
    if (!response.ok) throw new Error(`models.dev 目录请求失败（HTTP ${response.status}）`);
    return parseModelsDevCatalog(JSON.parse(await boundedText(response)));
  } catch (error) {
    if (controller.signal.aborted) throw new Error("models.dev 目录读取超时，请稍后手动重试");
    throw error;
  } finally {
    clearTimeout(timeout);
  }
}
