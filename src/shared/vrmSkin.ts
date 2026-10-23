/** VRM 与 PNG 包分别存储；界面只传摘要 ID，不传本机路径。 */
export interface VrmSkinModel {
  modelId: string; name: string; authors: string[]; bytes: number;
  meta: Record<string, unknown>;
  budget: { nodes: number; joints: number; triangles: number; decodedTextureBytes: number };
}
export interface VrmSkinState {
  selectedModelId: string | null; models: VrmSkinModel[];
  problems: { modelId: string | null; message: string }[];
}
export interface VrmSkinClient {
  state(): Promise<VrmSkinState>;
  chooseImport(): Promise<VrmSkinModel | null>;
  select(modelId: string | null): Promise<VrmSkinState>;
  remove(modelId: string): Promise<VrmSkinState>;
  readModel(modelId: string): Promise<ArrayBuffer>;
  subscribe(changed: (kind?: string) => void): Promise<() => void>;
}
const id = (value: unknown): value is string => typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
const text = (value: unknown, max: number): value is string => typeof value === "string" && value.trim().length > 0 &&
  value.length <= max && !/[\p{Cc}\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/u.test(value);
const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);
const integer = (value: unknown, max: number): value is number => Number.isSafeInteger(value) && Number(value) >= 0 && Number(value) <= max;
export function parseVrmModel(value: unknown): VrmSkinModel {
  if (!record(value) || !id(value.modelId) || !text(value.name, 256) || !integer(value.bytes, 64 * 1024 * 1024) || value.bytes < 32 ||
    !Array.isArray(value.authors) || !value.authors.length || value.authors.length > 32 || !value.authors.every(author => text(author, 256)) ||
    !record(value.meta) || !record(value.budget) || !integer(value.budget.nodes, 512) || !integer(value.budget.joints, 200) ||
    !integer(value.budget.triangles, 100_000) || !integer(value.budget.decodedTextureBytes, 96 * 1024 * 1024)) throw Error("VRM 模型摘要无效。");
  return structuredClone(value) as unknown as VrmSkinModel;
}
export function parseVrmState(value: unknown): VrmSkinState {
  if (!record(value) || !(value.selectedModelId === null || id(value.selectedModelId)) ||
    !Array.isArray(value.models) || value.models.length > 8 || !Array.isArray(value.problems) || value.problems.length > 32) throw Error("VRM 模型库无效。");
  const models = value.models.map(parseVrmModel);
  if (new Set(models.map(model => model.modelId)).size !== models.length ||
    (value.selectedModelId !== null && !models.some(model => model.modelId === value.selectedModelId))) throw Error("VRM 选择记录无效。");
  const problems = value.problems.map(problem => {
    if (!record(problem) || !(problem.modelId === null || id(problem.modelId)) || !text(problem.message, 2048)) throw Error("VRM 存储诊断无效。");
    return { modelId: problem.modelId, message: problem.message };
  });
  return { selectedModelId: value.selectedModelId, models, problems };
}
