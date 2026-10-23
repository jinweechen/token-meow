import test from "node:test";
import assert from "node:assert/strict";
import { parseObservedInsights } from "./observedInsights";

// 这些都是可控合成 DTO；不读取真实会话、账户或聊天正文。
const unknownCache = () => ({ inputTokens: null, cachedInputTokens: null, cacheReadPercent: null, knownResponses: 0, unknownResponses: 0 });
const pairedCache = () => ({ inputTokens: "100", cachedInputTokens: "20", cacheReadPercent: 20, knownResponses: 5, unknownResponses: 0 });
const model = (name: string) => ({ model: name, samples: 5, eligible: true, cache: pairedCache(), timedSamples: 5, meanTurnTimeToFirstTokenMs: 123, pricedSamples: 5, meanEstimatedCostUsd: "0.012345678" });
function fixture(): any {
  return { schemaVersion: 1, from: "2026-10-23", to: "2026-10-23", cache: pairedCache(),
    daily: [{ day: "2026-10-23", text: "合成本地响应摘要", recordedTokens: "9007199254740993", usage: { responseCount: 10 }, cache: { ...pairedCache(), knownResponses: 10 }, estimatedCostUsd: "9007199254740993.000001", pricedResponses: 10 }],
    timeline: [], timelineTruncated: false, comparisonTruncated: false, excludedComparisonResponses: 0,
    comparisons: [{ condition: { sourceId: "synthetic-source", providerId: "synthetic-provider", accountScope: "synthetic-hash", known: true, effort: "medium", speed: "fast", inputMagnitude: 3, cacheDecile: 2 },
      comparable: true, models: [model("synthetic-a"), model("synthetic-b")], quality: "conditional-observed", rank: null, reasons: ["observational_not_causal"] }] };
}
function rejects(change: (value: any) => void, message?: string) {
  const value = fixture(); change(value); assert.throws(() => parseObservedInsights(value), message);
}

test("不完整条件只能作描述，不能把条件完整标记当作替代证据", () => {
  const value = fixture(); const group = value.comparisons[0];
  group.condition = { ...group.condition, accountScope: null, providerId: "unknown", effort: "unknown", speed: "unknown", known: false };
  group.comparable = false;
  assert.equal(parseObservedInsights(value).comparisons[0].comparable, false);
  group.comparable = true; assert.throws(() => parseObservedInsights(value));
  group.condition.known = true; assert.throws(() => parseObservedInsights(value), "未知字段不能仅改 known=true 就成为可对照");
});

test("同模型的重复行不能伪装成两个独立模型达到可对照门槛", () => {
  rejects(v => v.comparisons[0].models[1].model = v.comparisons[0].models[0].model);
});

test("实际样本不足不发布可对照资格，两个模型中任一不足也不够", () => {
  const value = fixture(); const group = value.comparisons[0]; const row = group.models[1];
  row.samples = 4; row.eligible = false; row.timedSamples = 4; row.meanTurnTimeToFirstTokenMs = null; row.pricedSamples = 4; row.meanEstimatedCostUsd = null;
  assert.throws(() => parseObservedInsights(value));
  group.comparable = false; assert.equal(parseObservedInsights(value).comparisons[0].models[1].eligible, false);
  row.eligible = true; assert.throws(() => parseObservedInsights(value));
});

test("计时与价格的覆盖样本必须在总样本以内且不足5不发布均值", () => {
  for (const change of [
    (m: any) => m.timedSamples = 6,
    (m: any) => m.pricedSamples = 6,
    (m: any) => { m.timedSamples = 4; m.meanTurnTimeToFirstTokenMs = 0; },
    (m: any) => { m.pricedSamples = 4; m.meanEstimatedCostUsd = "0"; },
    (m: any) => m.meanTurnTimeToFirstTokenMs = Infinity,
    (m: any) => m.meanEstimatedCostUsd = "1e6",
  ]) rejects(v => change(v.comparisons[0].models[0]));
});

test("每日定价覆盖数不能超过记录响应数", () => {
  rejects(v => v.daily[0].pricedResponses = v.daily[0].usage.responseCount + 1);
});

test("Token与估算美元使用精确十进制字符串，不经JS浮点数压缩", () => {
  const value = fixture(); value.comparisons[0].models[0].meanEstimatedCostUsd = "9007199254740993.000000001";
  const parsed = parseObservedInsights(value);
  assert.equal(parsed.daily[0].recordedTokens, "9007199254740993");
  assert.equal(parsed.daily[0].estimatedCostUsd, "9007199254740993.000001");
  assert.equal(parsed.comparisons[0].models[0].meanEstimatedCostUsd, "9007199254740993.000000001");
});

test("压缩窗口序号可保留u64最大值，数值类型或非规范整数串拒绝", () => {
  const value = fixture(); value.timeline = [{ id: "synthetic-compaction", kind: "compaction", eventTime: "2026-10-23T04:00:00.000Z", sourceId: "synthetic-source", threadId: "synthetic-thread", windowId: null, windowNumber: "18446744073709551615" }];
  assert.equal(parseObservedInsights(value).timeline[0].windowNumber, "18446744073709551615");
  for (const bad of [18446744073709551615, -1, "01", "-1", "1.25", "1e3", {}]) {
    value.timeline[0].windowNumber = bad; assert.throws(() => parseObservedInsights(value));
  }
});

test("未知缓存保留null，与明确已知的0%区分", () => {
  const value = fixture(); value.cache = { ...unknownCache(), unknownResponses: 10 };
  assert.equal(parseObservedInsights(value).cache.cacheReadPercent, null);
  for (const partial of [{ inputTokens: "0" }, { cachedInputTokens: "0" }, { cacheReadPercent: 0 }]) {
    value.cache = { ...unknownCache(), unknownResponses: 10, ...partial }; assert.throws(() => parseObservedInsights(value));
  }
  value.cache = { inputTokens: "100", cachedInputTokens: "0", cacheReadPercent: 0, knownResponses: 5, unknownResponses: 5 };
  assert.equal(parseObservedInsights(value).cache.cacheReadPercent, 0, "明确的缓存0与未知不同");
});

test("成对有效缓存不能出现单边缺失或零分母仍发布0%", () => {
  rejects(v => v.cache = { ...pairedCache(), cachedInputTokens: null });
  rejects(v => v.cache = { ...pairedCache(), inputTokens: null });
  rejects(v => v.cache = { ...pairedCache(), inputTokens: "0", cachedInputTokens: "0", cacheReadPercent: 0 });
  rejects(v => v.cache = { ...pairedCache(), inputTokens: "10", cachedInputTokens: "11" });
  const value = fixture(); value.cache = { ...pairedCache(), inputTokens: "0", cachedInputTokens: "0", cacheReadPercent: null };
  assert.equal(parseObservedInsights(value).cache.cacheReadPercent, null);
});

test("隐私原生投影可为空时间线和对照组，同时缓存与日摘要仍为聚合数据", () => {
  // 仅检查实际parser接受隐私后端形状；组件隐私展示留给真实渲染测试，不读取源码做镜像断言。
  const value = fixture(); value.timeline = []; value.comparisons = [];
  value.reasons = ["privacy_mode", "local_recorded_only"];
  const parsed = parseObservedInsights(value);
  assert.deepEqual(parsed.timeline, []); assert.deepEqual(parsed.comparisons, []);
  assert.equal(parsed.daily[0].recordedTokens, "9007199254740993");
});

test("事件时间必须带时区，日期范围必须是真实有序日历日期", () => {
  rejects(v => v.from = "2026-02-30"); rejects(v => v.to = "2026-10-22");
  rejects(v => v.timeline = [{ id: "synthetic", kind: "compaction", eventTime: "2026-10-23T04:00:00", sourceId: "source", threadId: "thread", windowId: null, windowNumber: null }]);
});
