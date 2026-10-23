import assert from "node:assert/strict";
import test from "node:test";
import { parseVrmModel, parseVrmState } from "./vrmSkin";

const model = () => ({ modelId: "a".repeat(64), name: "示例角色", authors: ["作者"], bytes: 1024,
  meta: { name: "示例角色", licenseUrl: "https://vrm.dev/licenses/1.0/", commercialUsage: "personalNonProfit" },
  budget: { nodes: 10, joints: 5, triangles: 3, decodedTextureBytes: 16 } });
test("VRM 库保留模型许可且副本不受原输入修改影响", () => {
  const input = model();
  const parsed = parseVrmModel(input);
  input.meta.commercialUsage = "corporation";
  assert.equal(parsed.meta.commercialUsage, "personalNonProfit");
  assert.equal(parseVrmState({ selectedModelId: parsed.modelId, models: [parsed], problems: [] }).selectedModelId, parsed.modelId);
});
test("VRM 摘要拒绝路径标识、越界资源、重复库及悬空选择", () => {
  for (const value of [
    { ...model(), modelId: "C:\\avatar.vrm" }, { ...model(), name: "角色\u202eexe" }, { ...model(), authors: [] },
    { ...model(), bytes: 64 * 1024 * 1024 + 1 },
    { ...model(), budget: { ...model().budget, decodedTextureBytes: 96 * 1024 * 1024 + 1 } },
    { ...model(), budget: { ...model().budget, triangles: -1 } },
  ]) assert.throws(() => parseVrmModel(value));
  assert.throws(() => parseVrmState({ selectedModelId: "b".repeat(64), models: [model()], problems: [] }));
  assert.throws(() => parseVrmState({ selectedModelId: null, models: [model(), model()], problems: [] }));
});
