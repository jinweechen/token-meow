import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { SourceDetails } from "../features/sources/SourceDetails";
import { emptySnapshot } from "./demo";
import { SOURCE_KINDS, isSourceKind, sourceChannelName, sourceKindName } from "./sourceKind";

test("source tools use the canonical registration kinds and preserve unknown kinds explicitly", () => {
  assert.deepEqual(SOURCE_KINDS, ["codex", "claude", "gemini"]);
  assert.deepEqual(SOURCE_KINDS.map(sourceKindName), ["Codex", "Claude Code", "Gemini CLI"]);
  for (const unknown of ["claude_code", "gemini_cli", "", null, {}, "__proto__"])
    assert.equal(isSourceKind(unknown), false);
  assert.equal(sourceKindName("future"), "未识别工具（future）");
  assert.equal(sourceKindName(null), "工具未确认");
  assert.equal(sourceChannelName("json"), "JSON 本地文件");
  assert.equal(sourceChannelName("unknown"), "未识别通道");
  assert.equal(sourceChannelName("__proto__"), "未识别通道");
});

for (const kind of ["claude", "gemini"] as const) test(`${kind} log UI leaves account, activity, and task outcomes unproven`, () => {
  const snapshot = emptySnapshot();
  snapshot.primarySourceId = "codex-primary";
  snapshot.account = { sourceId: "codex-primary", status: "connected", collectedAt: new Date().toISOString(),
    buckets: [{ id: "official", name: "独立额度", primary: { usedPercent: 70, windowDurationMins: 300, resetsAt: null } }] };
  const source = { id: `${kind}-source`, root: "D:/private-selected-root", clientKind: kind, enabled: true, runtime: "可用" };
  snapshot.channels = [{ id: "local-log", sourceId: source.id, kind: kind === "gemini" ? "json" : "jsonl", enabled: true }];
  const html = renderToStaticMarkup(createElement(SourceDetails, { source, index: 1, snapshot,
    chooseSource: async () => {}, action: async () => {} }));
  for (const text of [sourceKindName(kind), "账号身份未知", "活动状态：未知", "任务结果：未知", "官方额度与账户日用量：未支持", "用量不证明任务成功或当前空闲", "不授予桌宠移动资格"])
    assert.ok(html.includes(text), text);
  assert.ok(!html.includes("70%"));
  assert.ok(!html.includes("主账户来源；快照单独读取"));
  assert.match(html, /disabled=""[^>]*>设为主账户来源/);
  assert.ok(html.includes(sourceChannelName(kind === "gemini" ? "json" : "jsonl")));
  snapshot.settings.privacyMode = true;
  const privateHtml = renderToStaticMarkup(createElement(SourceDetails, { source, index: 1, snapshot,
    chooseSource: async () => {}, choosingDirectory: true, action: async () => {} }));
  assert.ok(!privateHtml.includes(source.root));
  assert.ok(privateHtml.includes("来源路径已隐藏"));
  assert.match(privateHtml, /disabled=""[^>]*>更改目录/);
});
