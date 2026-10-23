import assert from "node:assert/strict";
import test from "node:test";
import type { PetPulse } from "./contracts";
import { checkExtraPetPulse, extraPetPresentation, extraPetWindowClient, parseExtraPetContext, parseExtraPetRenderer } from "../features/pet/useExtraPet";

const petId = "11111111-1111-4111-8111-111111111111";
const instanceEpoch = "22222222-2222-4222-8222-222222222222";
const rendererEpoch = "33333333-3333-4333-8333-333333333333";
const token = "44444444-4444-4444-8444-444444444444";
function context() { return { pet: { petId, instanceEpoch, label: `extra-pet-${petId}`, skinChoice: { kind: "builtin" }, visible: true, scale: 1, position: null }, nativeVisible: true, paused: false, png: null, vrm: null }; }
function renderer() { return { petId, instanceEpoch, rendererEpoch, token, generation: "9007199254740993" }; }

test("辅助视图上下文仅接受原生真实窗口登记和明确的暂停状态", () => {
  const raw = context(), parsed = parseExtraPetContext(raw);
  raw.pet.visible = false;
  assert.equal(parsed.pet.visible, true);
  for (const change of [
    (value: any) => { value.pet.label = "pet"; },
    (value: any) => { value.pet.petId = "../pet"; },
    (value: any) => { value.pet.scale = 2; },
    (value: any) => { value.nativeVisible = "true"; },
    (value: any) => { delete value.paused; },
    (value: any) => { value.png = []; },
  ]) { const value = context(); change(value); assert.throws(() => parseExtraPetContext(value)); }
});

test("辅助角色库缺失可回退内置，但已返回资源必须对应当前选择", () => {
  const raw: any = context();
  raw.pet.skinChoice = { kind: "vrm", id: "a".repeat(64) };
  assert.equal(parseExtraPetContext(raw).vrm, null);
  raw.vrm = { modelId: "b".repeat(64), name: "测试", authors: ["作者"], bytes: 32, meta: {}, budget: { nodes: 1, joints: 0, triangles: 0, decodedTextureBytes: 0 } };
  assert.throws(() => parseExtraPetContext(raw), /不匹配/);
  raw.vrm.modelId = "a".repeat(64);
  assert.equal(parseExtraPetContext(raw).vrm?.modelId, "a".repeat(64));
});

test("渲染许可必须匹配实例及请求代次，十进制代次不损失精度", () => {
  const ctx = parseExtraPetContext(context());
  assert.equal(parseExtraPetRenderer(renderer(), ctx, rendererEpoch).generation, "9007199254740993");
  for (const patch of [ { petId: instanceEpoch }, { instanceEpoch: petId }, { rendererEpoch: token }, { token: "../../token" },
    { generation: "01" }, { generation: "18446744073709551616" }, { generation: 1 }, { generation: "-1" } ]) {
    assert.throws(() => parseExtraPetRenderer({ ...renderer(), ...patch }, ctx, rendererEpoch));
  }
  assert.equal(parseExtraPetRenderer({ ...renderer(), generation: "18446744073709551615" }, ctx, rendererEpoch).generation, "18446744073709551615");
});

test("辅助视图拒绝任意通知提示且从不就地修改主桌宠设置", () => {
  const pulse = { settings: { petVisible: false }, prompts: [], activity: { activeTurns: 2 } } as unknown as PetPulse;
  assert.equal(checkExtraPetPulse(pulse), pulse);
  assert.throws(() => checkExtraPetPulse({ ...pulse, prompts: [{ id: "unexpected", kind: "completion", status: "completed", createdAt: new Date().toISOString(), animationEligible: true }] }));
  const visible = extraPetPresentation(pulse, true)!;
  assert.equal(visible.settings.petVisible, true);
  assert.equal(pulse.settings.petVisible, false);
  assert.equal(visible.activity.activeTurns, 2);
  assert.deepEqual(visible.prompts, []);
  assert.equal(extraPetPresentation(pulse, false)?.settings.petVisible, false);
  assert.equal(extraPetPresentation(null, true), null);
});

test("生产 IPC 只提交窗口许可和 opaque 素材 ID，不提交 URL 身份或主桌宠命令", async () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
  const calls: { command: string; args: any }[] = [];
  const callbacks = new Map<number, (event: unknown) => void>();
  let next = 0, received: unknown;
  Object.defineProperty(globalThis, "window", { configurable: true, value: {
    __TAURI_INTERNALS__: {
      transformCallback(callback: (event: unknown) => void) { callbacks.set(++next, callback); return next; },
      async invoke(command: string, args: any) {
        calls.push({ command, args });
        if (command === "read_pet_skin_frame") return [137, 80, 78, 71];
        if (command === "read_vrm_skin_model") return new Uint8Array([103, 108, 84, 70]).buffer;
        if (command === "plugin:event|listen") return 7;
        return null;
      },
    },
    __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
  } });
  try {
    const lease = renderer();
    await extraPetWindowClient.context(); await extraPetWindowClient.register(rendererEpoch);
    await extraPetWindowClient.unregister(rendererEpoch, token); await extraPetWindowClient.pulse(null);
    const request = { clientX: 12, clientY: 34, viewportWidth: 280, viewportHeight: 340 };
    await extraPetWindowClient.drag(request, lease); await extraPetWindowClient.close(lease);
    assert.deepEqual([...new Uint8Array(await extraPetWindowClient.readFrame("a".repeat(64), "frame-0"))], [137, 80, 78, 71]);
    assert.deepEqual([...new Uint8Array(await extraPetWindowClient.readModel("b".repeat(64)))], [103, 108, 84, 70]);
    const remove = await extraPetWindowClient.subscribe("extra-pets-changed", payload => { received = payload; });
    callbacks.get(next)!({ payload: { kind: "configured" } }); remove();
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.deepEqual(received, { kind: "configured" });
    assert.deepEqual(calls.map(call => call.command), ["extra_pet_context", "register_extra_pet_renderer", "unregister_extra_pet_renderer", "extra_pet_pulse", "start_extra_pet_drag", "close_extra_pet", "read_pet_skin_frame", "read_vrm_skin_model", "plugin:event|listen", "plugin:event|unlisten"]);
    assert.deepEqual(calls[4].args, { request, rendererEpoch, token });
    assert.deepEqual(calls[5].args, { rendererEpoch, token });
    assert.deepEqual(calls[6].args, { skinId: "a".repeat(64), frameId: "frame-0" });
    assert.deepEqual(calls[7].args, { modelId: "b".repeat(64) });
  } finally {
    if (descriptor) Object.defineProperty(globalThis, "window", descriptor);
    else Reflect.deleteProperty(globalThis, "window");
  }
});
