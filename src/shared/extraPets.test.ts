import assert from "node:assert/strict";
import test from "node:test";
import { parseExtraPet } from "./extraPets";
const pet = () => ({ petId: "00000000-0000-4000-8000-000000000001", instanceEpoch: "00000000-0000-4000-8000-000000000002",
  label: "extra-pet-00000000-0000-4000-8000-000000000001", skinChoice: { kind: "builtin" }, visible: true, scale: 1, position: { x: -100, y: 20 } });
test("辅助桌宠身份由精确本机登记映射，并复制返回配置", () => {
  const input = pet(); const parsed = parseExtraPet(input); input.position.x = 30;
  assert.equal(parsed.position?.x, -100);
});
test("辅助视图拒绝伪装主窗口、路径角色和无界缩放/位置", () => {
  for (const override of [{ label: "pet" }, { label: "extra-pet-other" }, { instanceEpoch: "old" },
    { skinChoice: { kind: "vrm", id: "C:\\model.vrm" } }, { scale: 0 }, { scale: 2 }, { visible: null }, { position: { x: NaN, y: 0 } }]) {
    assert.throws(() => parseExtraPet({ ...pet(), ...override }));
  }
});
