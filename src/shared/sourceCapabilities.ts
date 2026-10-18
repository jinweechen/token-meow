import type { FieldCapability, Snapshot, SourceInstance } from "./contracts";
import { accountExpired } from "./format";

export const capabilityFields = [
  ["usage", "单响应用量"],
  ["cacheRead", "缓存读取"],
  ["cacheWrite", "缓存写入"],
  ["reasoning", "推理输出"],
  ["turnCompletion", "回合结束"],
  ["turnTiming", "回合计时"],
  ["requestTiming", "请求计时 / 生成速度"],
  ["subagents", "子代理关系"],
  ["context", "上下文快照"],
  ["chatTarget", "聊天入口"],
  ["quota", "官方账户额度"],
  ["accountUsage", "官方账户日用量"],
] as const;

export function capabilityGranularity(value: string | null | undefined) {
  if (!value) return "未验证";
  return (
    {
      response: "单响应",
      turn: "回合",
      request: "单请求",
      "thread-relationship": "线程父子关系",
      "context-snapshot": "上下文快照",
      thread: "会话",
      "quota-window": "账户额度窗口",
      "account-day": "账户日桶（官方日期）",
    }[value] ?? value
  );
}

/** A declaration and a readable JSONL directory cannot prove account RPC health. */
export function capabilityRuntime(
  key: (typeof capabilityFields)[number][0],
  field: FieldCapability | undefined,
  source: SourceInstance,
  snapshot: Snapshot,
  now = Date.now(),
) {
  if (key !== "quota" && key !== "accountUsage")
    return field?.runtime ?? "未验证";
  if (!source.enabled || source.id !== snapshot.primarySourceId)
    return "未配置";
  const account = snapshot.account;
  if (!account || account.sourceId !== source.id) return "未验证";
  if (accountExpired(account, now)) return "已过期";
  if (account.status === "unavailable") return "不可用";
  const collectedAt = Date.parse(account.collectedAt ?? "");
  if (
    account.status !== "connected" ||
    !Number.isFinite(collectedAt) ||
    collectedAt > now
  )
    return "未验证";
  const provided =
    key === "quota"
      ? account.buckets.some((bucket) =>
          [bucket.primary, bucket.secondary].some(
            (window) =>
              window?.usedPercent != null &&
              Number.isFinite(window.usedPercent),
          ),
        )
      : Array.isArray(account.usage?.dailyUsageBuckets);
  return provided ? "可用" : "未验证";
}
