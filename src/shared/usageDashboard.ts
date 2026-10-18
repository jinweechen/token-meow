/** Presentation helpers keep missing usage and prices distinct from observed zero. */
export function usageNumber(
  value: number | null | undefined,
  compact = false,
): string {
  if (value == null || !Number.isFinite(value)) return "未提供";
  if (!compact) return new Intl.NumberFormat("zh-CN").format(value);
  const units: [number, string][] = [
    [1e9, "B"],
    [1e6, "M"],
    [1e3, "K"],
  ];
  const unit = units.find(([size]) => Math.abs(value) >= size);
  return unit
    ? `${new Intl.NumberFormat("en-US", { maximumFractionDigits: 2 }).format(value / unit[0])}${unit[1]}`
    : String(value);
}

export function usageUsd(value: string | null | undefined): string {
  if (value == null || value.trim() === "") return "未定价";
  const amount = Number(value);
  if (!Number.isFinite(amount) || amount < 0) return "未提供";
  if (amount > 0 && amount < 0.00000001) return "< $0.00000001";
  return `$${new Intl.NumberFormat("en-US", {
    minimumFractionDigits: 2,
    maximumFractionDigits: amount > 0 && amount < 0.01 ? 8 : 6,
  }).format(amount)}`;
}

export function usageTime(
  value: string | null | undefined,
  withDate = false,
): string {
  if (!value) return "未提供";
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "时间不可用";
  return new Intl.DateTimeFormat("zh-CN", {
    timeZone: "Asia/Shanghai",
    hour12: false,
    ...(withDate
      ? ({ year: "numeric", month: "2-digit", day: "2-digit" } as const)
      : {}),
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(date);
}

export function usageDay(value: string | null | undefined): string {
  if (!value) return "未提供";
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "时间不可用";
  return new Intl.DateTimeFormat("zh-CN", {
    timeZone: "Asia/Shanghai",
    month: "2-digit",
    day: "2-digit",
  }).format(date);
}

export function priceReasonLabel(reason: string | null | undefined): string {
  if (!reason) return "未匹配有效价格";
  return (
    (
      {
        missing_price: "未匹配有效价格",
        price_not_found: "未匹配有效价格",
        no_price: "未匹配有效价格",
        unknown_model: "模型未知",
        missing_model: "模型未知",
        model_unknown: "模型未知",
        missing_usage: "用量字段不完整",
        incomplete_usage: "用量字段不完整",
        missing_cache_write: "缺少缓存写入用量",
        invalid_usage: "计量无效",
        conflict: "计量存在冲突",
        invalid_meter: "计量无效",
        unsupported_usage_semantics: "来源用量语义尚不支持",
        incomplete_or_invalid_token_counts: "必要用量缺失或异常",
        inconsistent_token_counts: "用量计数关系异常",
        incomplete_or_invalid_native_counts: "原始计数缺失或冲突",
        invalid_event_time: "事件时间异常",
        no_matching_price: "未匹配有效价格",
        missing_cache_read_price: "缓存读取量非零，但缺少对应价格",
        missing_cache_write_price: "缓存写入量非零，但缺少对应价格",
      } as Record<string, string>
    )[reason] ?? reason
  );
}

/** The editor accepts plain, nonnegative decimal prices, not exponent notation. */
export function validPriceDecimal(value: string): boolean {
  return (
    /^(?:0|[1-9]\d*)(?:\.\d{1,6})?$/.test(value.trim()) &&
    Number.isFinite(Number(value)) &&
    Number(value) <= 1_000_000
  );
}

/** datetime-local fields on this panel explicitly represent Shanghai wall time. */
export function shanghaiDateTimeInput(now = new Date()): string {
  if (!Number.isFinite(now.getTime())) return "";
  return new Date(now.getTime() + 8 * 60 * 60 * 1000)
    .toISOString()
    .slice(0, 16);
}

export function shanghaiEffectiveFrom(value: string): string | null {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(value)) return null;
  const date = new Date(`${value}:00+08:00`);
  return Number.isFinite(date.getTime()) &&
    shanghaiDateTimeInput(date) === value
    ? `${value}:00+08:00`
    : null;
}
