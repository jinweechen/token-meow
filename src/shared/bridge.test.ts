import assert from "node:assert/strict";
import test from "node:test";
import { openThread, exportCsv, setPetInteraction, openPanel, panelNavigation, openPetChat } from "./bridge";

test("pet shortcut bridge sends only finite request envelopes and preserves ordinary IPC rejection", async () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "window");
  const calls: { command: string; args: unknown }[] = [];
  let reject = false;
  Object.defineProperty(globalThis, "window", { configurable: true, value: {
    __TAURI_INTERNALS__: { invoke: async (command: string, args: unknown) => {
      calls.push({ command, args });
      if (reject) throw Error("domain rejected");
      return command === "open_pet_chat" ? { status: "submitted" } : { watermark: "9007199254740993", pending: null };
    } },
  } });
  const petDomain = { day: "2026-10-17", runtimeEpoch: "runtime", sourceScopeRevision: "scope" };
  try {
    await openPanel();
    await openPanel({ destination: "inbox", petDomain });
    await panelNavigation({ action: "peek" });
    await panelNavigation({ action: "ack", revision: "9007199254740993" });
    await panelNavigation({ action: "cancel" });
    const request = { petDomain, chat: { sourceId: "source", threadId: "thread" } };
    assert.deepEqual(await openPetChat(request), { status: "submitted" });
    assert.deepEqual(calls, [
      { command: "open_panel", args: { request: null } },
      { command: "open_panel", args: { request: { destination: "inbox", petDomain } } },
      { command: "panel_navigation", args: { request: { action: "peek" } } },
      { command: "panel_navigation", args: { request: { action: "ack", revision: "9007199254740993" } } },
      { command: "panel_navigation", args: { request: { action: "cancel" } } },
      { command: "open_pet_chat", args: { request } },
    ]);
    reject = true;
    await assert.rejects(() => openPetChat(request), /domain rejected/);
  } finally {
    if (original) Object.defineProperty(globalThis, "window", original);
    else Reflect.deleteProperty(globalThis, "window");
  }
});

test("production finite bridge keeps source scope in chat payload and validates nullable CSV return", async () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "window");
  const calls: { command: string; args: unknown }[] = [];
  let result: unknown = null;
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      __TAURI_INTERNALS__: {
        invoke: async (command: string, args: unknown) => {
          calls.push({ command, args });
          return result;
        },
      },
    },
  });
  try {
    await openThread("source-b", "same-thread");
    assert.deepEqual(calls[0], {
      command: "open_thread",
      args: { sourceId: "source-b", threadId: "same-thread" },
    });
    assert.equal(await exportCsv({ project: "path:d:/one/app" }), null);
    assert.deepEqual(calls[1], {
      command: "export_csv",
      args: { query: { project: "path:d:/one/app" } },
    });
    result = "用量.csv";
    assert.equal(await exportCsv({}), "用量.csv");
    result = { cancelled: true };
    await assert.rejects(() => exportCsv({}), /未确认导出成功/);
    result = { active: true, revision: 123 };
    assert.deepEqual(
      await setPetInteraction({ active: true, revision: 123 }),
      result,
    );
    assert.deepEqual(calls.at(-1), {
      command: "set_pet_interaction",
      args: { active: true, revision: 123 },
    });
  } finally {
    if (original) Object.defineProperty(globalThis, "window", original);
    else Reflect.deleteProperty(globalThis, "window");
  }
});
