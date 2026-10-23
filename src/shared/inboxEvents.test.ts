import assert from "node:assert/strict";
import test from "node:test";
import { createElement, isValidElement, type ReactElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { InboxEventRow, RecentInboxEvent, inboxEventLabel, inboxEventSuccess } from "../features/inbox/InboxEvent";
import { promptAction, promptText } from "../features/pet/presenter";
import { PromptWatermark } from "../features/pet/promptWatermark";
import { projectSnapshotPresentation } from "../features/pet/petProjection";
import type { InboxEvent } from "./contracts";
import { emptySnapshot } from "./demo";

function planEvent(): InboxEvent {
  // Exact subscription_plan envelope: plans have no root-turn result status.
  return { id: "plan-reminder-a", kind: "subscription_plan", sourceId: "source-a", threadId: "", turnId: null,
    title: "使用计划达到 80%", message: "私有计划甲 已达到计划的 80%", createdAt: "2026-10-22T10:00:00Z", read: false };
}
function buttons(node: ReactNode): ReactElement<{ children?: ReactNode; onClick?: () => void }>[] {
  if (Array.isArray(node)) return node.flatMap(buttons);
  if (!isValidElement<{ children?: ReactNode; onClick?: () => void }>(node)) return [];
  return [...(node.type === "button" ? [node] : []), ...buttons(node.props.children)];
}
function buttonText(node: ReactNode): string {
  if (Array.isArray(node)) return node.map(buttonText).join("");
  if (isValidElement<{ children?: ReactNode }>(node)) return buttonText(node.props.children);
  return typeof node === "string" ? node : "";
}

test("a real status-less plan reminder renders its own details and usable read/plan actions", () => {
  const event = planEvent();
  const reads: string[] = [], chats: string[] = []; let openedPlan = 0;
  const props = { event, index: 0, privacy: false, canViewPlan: true, onRead: (id: string) => reads.push(id), onChat: (id: string) => chats.push(id), onPlan: () => { openedPlan++; } };
  const html = renderToStaticMarkup(createElement(InboxEventRow, props));
  assert.ok(html.includes("使用计划提醒")); assert.ok(html.includes("私有计划甲 已达到计划的 80%"));
  assert.ok(html.includes("标为已读")); assert.ok(html.includes("查看计划"));
  assert.ok(!html.includes("回合结束") && !html.includes("结果未知") && !html.includes("返回聊天"));
  const actions = buttons(InboxEventRow(props));
  actions.find((b) => buttonText(b) === "标为已读")!.props.onClick!();
  actions.find((b) => buttonText(b) === "查看计划")!.props.onClick!();
  assert.deepEqual(reads, [event.id]); assert.equal(openedPlan, 1); assert.deepEqual(chats, []);
});

test("private and historical-source plan reminders keep read actions without wrong-source navigation", () => {
  const event = planEvent();
  const html = renderToStaticMarkup(createElement(InboxEventRow, { event, index: 2, privacy: true, canViewPlan: false, onRead() {}, onChat() {}, onPlan() {} }));
  assert.ok(html.includes("私密计划 3") && html.includes("使用计划提醒"));
  assert.ok(html.includes("标为已读")); assert.ok(!html.includes("查看计划"));
  assert.ok(!html.includes("私有计划甲") && !html.includes("使用计划达到 80%") && !html.includes("私密会话"));
});

test("recent plan or source-wide reminders open the inbox and never issue an empty chat request", () => {
  const chats: string[] = []; let inbox = 0;
  const onChat = (id: string) => chats.push(id), onInbox = () => { inbox++; };
  for (const event of [planEvent(), { ...planEvent(), threadId: "unexpected-plan-thread" }, { ...planEvent(), kind: "budget" }]) {
    const row = RecentInboxEvent({ event, privacy: false, onChat, onInbox });
    (row.props as { onClick: () => void }).onClick();
  }
  assert.equal(inbox, 3); assert.deepEqual(chats, []);
  const normal = { ...planEvent(), kind: "completion", status: "completed", threadId: "native-thread-a" };
  const row = RecentInboxEvent({ event: normal, privacy: false, onChat, onInbox });
  (row.props as { onClick: () => void }).onClick();
  assert.deepEqual(chats, ["native-thread-a"]); assert.equal(inbox, 3);
  assert.equal(inboxEventLabel(normal), "正常结束"); assert.equal(inboxEventSuccess(normal), true);
  assert.equal(inboxEventSuccess(planEvent()), false);
});

test("desktop-pet projection and presenter retain the plan domain without completion text", () => {
  const event = { ...planEvent(), animationEligible: true };
  const snapshot = emptySnapshot(); snapshot.inbox = [event];
  const projection = projectSnapshotPresentation(snapshot);
  assert.equal(projection.prompts[0].status, "unknown"); assert.equal(projection.prompts[0].kind, "subscription_plan");
  const watermark = new PromptWatermark(); const now = Date.parse(event.createdAt);
  watermark.consume([], now, false);
  const prompts = watermark.consume([event], now, false);
  assert.equal(prompts.length, 1);
  assert.equal(promptAction(prompts[0]), "alert");
  assert.equal(promptText(prompts[0]), "使用计划达到提醒目标，可打开收件箱查看");
  assert.ok(!promptText(prompts[0]).includes("结束"));
  assert.equal(promptAction({ ...prompts[0], status: "completed" }), "alert", "a reminder cannot become a completion celebration");
});

test("quota-watch warnings keep their quota meaning and never navigate to a chat", () => {
  const event = { ...planEvent(), kind: "quota_eta", title: "额度风险", message: "按近期活动可能在重置前触限", threadId: "unexpected" };
  let opened = 0;const chats: string[] = [];
  const props = { event, index: 0, privacy: false, canViewPlan: true, onRead() {}, onChat: (id: string) => chats.push(id), onPlan: () => { opened++; } };
  const html = renderToStaticMarkup(createElement(InboxEventRow, props));
  assert.ok(html.includes("官方额度风险") && html.includes("查看额度"));assert.ok(!html.includes("回合结束") && !html.includes("返回聊天"));
  buttons(InboxEventRow(props)).find(b => buttonText(b) === "查看额度")!.props.onClick!();assert.equal(opened, 1);assert.deepEqual(chats, []);
  const prompt = { id: event.id, kind: "quota_eta", status: "completed", createdAt: Date.parse(event.createdAt) };
  assert.equal(promptAction(prompt), "alert");assert.ok(promptText(prompt).includes("重置前触限"));
});
