export const SOURCE_KINDS = ["codex", "claude", "gemini"] as const;
export type SourceKind = (typeof SOURCE_KINDS)[number];

const definitions: Record<SourceKind, { name: string; mark: string; log: string }> = {
  codex: { name: "Codex", mark: "C", log: "Codex 会话 JSONL；仅采集适配器明确支持的字段。" },
  claude: { name: "Claude Code", mark: "A", log: "Claude Code 会话 JSONL；按响应用量记录，不代表官方订阅额度。" },
  gemini: { name: "Gemini CLI", mark: "G", log: "Gemini CLI 会话 JSONL / JSON；按响应用量记录，不代表官方订阅额度。" },
};

export function isSourceKind(value: unknown): value is SourceKind {
  return typeof value === "string" && SOURCE_KINDS.some(kind => kind === value);
}
export function sourceKindName(value: string | null | undefined): string {
  return isSourceKind(value) ? definitions[value].name : value ? `未识别工具（${value}）` : "工具未确认";
}
export function sourceKindMark(value: string): string {
  return isSourceKind(value) ? definitions[value].mark : "?";
}
export function sourceLogDescription(value: string): string {
  return isSourceKind(value) ? definitions[value].log : "日志格式和字段口径尚未验证。";
}
export function sourceHasUsageOnlyEvidence(value: string): boolean {
  return value === "claude" || value === "gemini";
}
export function sourceChannelName(value: string): string {
  switch (value) {
    case "jsonl": return "JSONL 本地文件";
    case "json": return "JSON 本地文件";
    case "app-server": return "app-server 只读账户通道";
    default: return "未识别通道";
  }
}
