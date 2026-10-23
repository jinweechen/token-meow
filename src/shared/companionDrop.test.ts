import assert from "node:assert/strict";
import test from "node:test";
import { parseCompanionDrop } from "./companionDrop";

const batch = () => ({ batchId: "00000000-0000-4000-8000-000000000001", kind: "openFiles", fileNames: ["报告 & $ (一).txt"], expiresInMs: 60_000 });
test("拖放只展示不透明批次与文件名，复制数组避免外部修改", () => {
  assert.equal(parseCompanionDrop(null), null);
  const original = batch(); const parsed = parseCompanionDrop(original)!;
  assert.deepEqual(parsed, original);
  original.fileNames.push("other.txt"); assert.equal(parsed.fileNames.length, 1);
});
test("路径、控制字符、未知字段及不可用租约不能进入确认界面", () => {
  for (const override of [
    { batchId: "../path" }, { kind: "runCommand" }, { fileNames: [] },
    { fileNames: ["C:\\source\\file.txt"] }, { fileNames: ["../file"] }, { fileNames: ["bad\u202efile"] },
    { fileNames: ["bad\nfile"] }, { fileNames: Array(9).fill("file.txt") },
    { expiresInMs: 0 }, { expiresInMs: 60_001 }, { expiresInMs: 1.5 }, { path: "C:\\x" },
    { kind: "registerExecutable", fileNames: ["one.exe", "two.exe"] },
  ]) assert.throws(() => parseCompanionDrop({ ...batch(), ...override }));
});
