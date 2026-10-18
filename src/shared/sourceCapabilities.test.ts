import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { SourceDetails } from "../features/sources/SourceDetails";
import { emptySnapshot } from "./demo";
import { capabilityRuntime } from "./sourceCapabilities";
import type { AccountSnapshot, SourceInstance } from "./contracts";

const now = Date.parse("2026-10-15T14:00:00Z");
const source: SourceInstance = {
  id: "source-one",
  root: "D:/private-codex",
  clientKind: "codex",
  enabled: true,
  runtime: "可用",
};

test("source capability UI names fixed verification scope and leaves missing metadata unverified", () => {
  const snapshot = emptySnapshot();
  snapshot.primarySourceId = source.id;
  snapshot.sourceCapabilities = [
    {
      sourceId: source.id,
      usage: {
        support: "本机已验证",
        runtime: null,
        supportedVersion: "0.147.0",
        testedVersion: "0.147.0",
        channel: "jsonl",
        granularity: "response",
        prerequisites: ["来源启用", "JSONL 可读"],
        evidenceRefs: ["docs/acceptance/scoped-evidence.json"],
        verifiedAt: "2026-10-15",
        verificationScope: "六份脱敏样本；不代表全部格式",
      },
    },
  ];
  const html = renderToStaticMarkup(
    createElement(SourceDetails, {
      source,
      index: 0,
      snapshot,
      chooseSource: async () => {},
      action: async () => {},
    }),
  );
  for (const text of [
    "支持版本：0.147.0",
    "实测版本：0.147.0",
    "最近验证：2026-10-15",
    "采集通道：jsonl",
    "来源启用；JSONL 可读",
    "六份脱敏样本；不代表全部格式",
    "docs/acceptance/scoped-evidence.json",
    "支持版本：未验证",
    "最近验证：未验证",
    "能力运行态：未验证",
    "官方账户额度",
    "官方账户日用量",
    "回合结束",
  ])
    assert.ok(html.includes(text), text);
});

test("account capability runtime requires this exact primary source and a fresh supplied field; zero is valid", () => {
  const snapshot = emptySnapshot();
  snapshot.primarySourceId = source.id;
  snapshot.account = {
    sourceId: source.id,
    status: "connected",
    collectedAt: new Date(now).toISOString(),
    buckets: [
      {
        id: "bucket",
        name: "额度",
        primary: {
          usedPercent: 0,
          windowDurationMins: 300,
          resetsAt: now / 1000 + 60,
        },
      },
    ],
    usage: { dailyUsageBuckets: [] },
  };
  const runtime = (key: "quota" | "accountUsage", item = source, at = now) =>
    capabilityRuntime(key, { runtime: null }, item, snapshot, at);
  assert.equal(runtime("quota"), "可用");
  assert.equal(runtime("accountUsage"), "可用");
  snapshot.account.usage = {
    summary: {
      lifetimeTokens: null,
      peakDailyTokens: null,
      longestRunningTurnSec: null,
      currentStreakDays: null,
      longestStreakDays: null,
    },
    dailyUsageBuckets: null,
  };
  assert.equal(runtime("accountUsage"), "未验证");
  assert.equal(runtime("quota", { ...source, id: "second-source" }), "未配置");
  assert.equal(runtime("quota", { ...source, enabled: false }), "未配置");
  snapshot.account.sourceId = "previous-primary";
  assert.equal(runtime("quota"), "未验证");
  snapshot.account.sourceId = source.id;
  assert.equal(runtime("quota", source, now + 61_000), "已过期");
  snapshot.account.buckets[0].primary!.resetsAt = null;
  assert.equal(runtime("quota", source, now + 241_000), "已过期");
  snapshot.account.buckets[0].primary!.usedPercent = null;
  assert.equal(runtime("quota"), "未验证");
  snapshot.account.usage = null;
  assert.equal(runtime("accountUsage"), "未验证");
  snapshot.account.status = "unavailable";
  assert.equal(runtime("quota"), "不可用");
  snapshot.account.status = "connected";
  snapshot.account.collectedAt = null;
  assert.equal(runtime("quota"), "已过期");
  assert.equal(
    capabilityRuntime("usage", { runtime: null }, source, snapshot, now),
    "未验证",
  );
  assert.equal(
    capabilityRuntime(
      "requestTiming",
      { runtime: "不可用" },
      source,
      snapshot,
      now,
    ),
    "不可用",
  );
});

test("account capability freshness uses the shared snapshot contract for invalid time and exact expiry", () => {
  const snapshot = emptySnapshot();
  snapshot.primarySourceId = source.id;
  const account: AccountSnapshot = {
    sourceId: source.id, status: "connected", collectedAt: new Date(now).toISOString(),
    buckets: [{ id: "quota", name: "额度", primary: { usedPercent: 0, windowDurationMins: 300, resetsAt: null } }],
    usage: { dailyUsageBuckets: [] },
  };
  snapshot.account = account;
  for (const key of ["quota", "accountUsage"] as const) {
    for (const collectedAt of [null, "invalid", new Date(now + 1).toISOString()]) {
      account.collectedAt = collectedAt;
      assert.equal(capabilityRuntime(key, undefined, source, snapshot, now), "已过期", `${key}: ${collectedAt}`);
    }
    account.collectedAt = new Date(now - 239_999).toISOString();
    assert.equal(capabilityRuntime(key, undefined, source, snapshot, now), "可用");
    account.collectedAt = new Date(now - 240_000).toISOString();
    assert.equal(capabilityRuntime(key, undefined, source, snapshot, now), "已过期");
    account.collectedAt = new Date(now).toISOString();
    account.status = "stale";
    assert.equal(capabilityRuntime(key, undefined, source, snapshot, now), "已过期");
    account.status = "connected";
  }
  snapshot.account = { sourceId: source.id, status: "loading", collectedAt: null, buckets: [] };
  assert.equal(capabilityRuntime("quota", undefined, source, snapshot, now), "未验证");
  snapshot.account.status = "unavailable";
  assert.equal(capabilityRuntime("quota", undefined, source, snapshot, now), "不可用");
});

test("source capability evidence and source path remain hidden in privacy mode", () => {
  const snapshot = emptySnapshot();
  snapshot.settings.privacyMode = true;
  snapshot.sourceCapabilities = [
    {
      sourceId: source.id,
      usage: { evidenceRefs: ["docs/private-evidence-path.json"] },
    },
  ];
  const html = renderToStaticMarkup(
    createElement(SourceDetails, {
      source,
      index: 0,
      snapshot,
      chooseSource: async () => {},
      action: async () => {},
    }),
  );
  assert.ok(!html.includes(source.root));
  assert.ok(!html.includes("private-evidence-path"));
  assert.ok(html.includes("隐私模式已隐藏"));
});
