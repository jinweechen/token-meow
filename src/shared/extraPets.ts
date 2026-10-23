import { invoke } from "@tauri-apps/api/core";
export type ExtraSkin = { kind: "builtin" } | { kind: "png" | "vrm"; id: string };
export interface ExtraPetIdentity { petId: string; instanceEpoch: string }
export interface ExtraPet extends ExtraPetIdentity {
  label: string; skinChoice: ExtraSkin; visible: boolean; scale: number; position: { x: number; y: number } | null;
}
const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/;
export function parseExtraPet(input: unknown): ExtraPet {
  const value = input as ExtraPet;
  const skin = value?.skinChoice;
  if (!value || typeof value !== "object" || !uuid.test(value.petId) || !uuid.test(value.instanceEpoch) ||
    value.label !== `extra-pet-${value.petId}` || typeof value.visible !== "boolean" || !Number.isFinite(value.scale) || value.scale < .5 || value.scale > 1.5 ||
    !skin || !(skin.kind === "builtin" || ((skin.kind === "png" || skin.kind === "vrm") && /^[a-f0-9]{64}$/.test(skin.id))) ||
    !(value.position === null || (Number.isInteger(value.position?.x) && Number.isInteger(value.position?.y) &&
      Math.abs(value.position.x) <= 2147483647 && Math.abs(value.position.y) <= 2147483647))) throw Error("辅助桌宠登记无效。");
  return structuredClone(value);
}
export const extraPetClient = {
  async list() { const values = await invoke<unknown[]>("list_extra_pets"); if (!Array.isArray(values) || values.length > 1) throw Error("辅助桌宠数量无效。"); return values.map(parseExtraPet); },
  async add(skinChoice: ExtraSkin) { return parseExtraPet(await invoke("create_extra_pet", { config: { skinChoice, visible: true, scale: 1 } })); },
  async configure(identity: ExtraPetIdentity, patch: { skinChoice?: ExtraSkin; visible?: boolean; scale?: number }) { return parseExtraPet(await invoke("configure_extra_pet", { identity: { petId: identity.petId, instanceEpoch: identity.instanceEpoch }, patch })); },
  remove: (identity: ExtraPetIdentity) => invoke<void>("remove_extra_pet", { identity: { petId: identity.petId, instanceEpoch: identity.instanceEpoch } }),
};
