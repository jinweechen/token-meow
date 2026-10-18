export function parseExportResult(value: unknown): string | null {
  if (value === null) return null;
  if (typeof value === "string" && value.length > 0) return value;
  throw Error("导出返回格式不兼容，请检查应用版本；未确认导出成功。");
}
export const exportResultMessage = (value: string | null): string | null =>
  value === null ? null : `已导出：${value}`;
