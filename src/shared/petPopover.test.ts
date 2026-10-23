import assert from "node:assert/strict";
import test from "node:test";
import {
  PetPopoverController,
  type PetPopover,
  type PetPopoverHost,
} from "./petPopover";

class TrackedTarget extends EventTarget {
  hidden = false;
  listeners = 0;
  override addEventListener(
    ...args: Parameters<EventTarget["addEventListener"]>
  ) {
    this.listeners++;
    super.addEventListener(...args);
  }
  override removeEventListener(
    ...args: Parameters<EventTarget["removeEventListener"]>
  ) {
    this.listeners--;
    super.removeEventListener(...args);
  }
}
function setup(overrides: Partial<PetPopoverHost> = {}) {
  const window = new TrackedTarget(),
    document = new TrackedTarget();
  const calls: { active: boolean; revision: number }[] = [];
  const values: PetPopover[] = [];
  const errors: unknown[] = [];
  let nativeDismiss: ((event: { revision?: number }) => void) | undefined;
  let nativeListeners = 0;
  const controller = new PetPopoverController({
    window,
    document,
    change: (value) => values.push(value),
    interrupt: () => undefined,
    error: (error) => errors.push(error),
    interaction: async (value) => {
      calls.push(value);
      return value;
    },
    listenDismiss: async (callback) => {
      nativeDismiss = callback;
      nativeListeners++;
      return () => {
        nativeListeners--;
      };
    },
    ...overrides,
  });
  return {
    controller,
    window,
    document,
    calls,
    values,
    errors,
    dismiss: (revision?: number) => nativeDismiss?.({ revision }),
    nativeListeners: () => nativeListeners,
  };
}
const settle = async () => {
  await Promise.resolve();
  await Promise.resolve();
};

test("窗口动作等原生关闭完成，重开或关闭失败不能启动旧动作", async () => {
  const pending: { value: { active: boolean; revision: number }; resolve: (value: { active: boolean; revision: number }) => void }[] = [];
  const fixture = setup({ interaction: value => new Promise(resolve => pending.push({ value, resolve })) });
  fixture.controller.toggle("menu");
  let finished = false;
  const closed = fixture.controller.closeForAction().then(value => { finished = true; return value; });
  await settle();
  assert.equal(finished, false);
  pending[1].resolve(pending[1].value);
  assert.equal(await closed, true);
  fixture.controller.toggle("menu");
  const superseded = fixture.controller.closeForAction();
  fixture.controller.toggle("card");
  pending[3].resolve(pending[3].value);
  assert.equal(await superseded, false);
  const denied = fixture.controller.closeForAction();
  pending[5].resolve({ ...pending[5].value, active: true });
  assert.equal(await denied, false);
  fixture.controller.dispose();
});

test("production popover focus lease closes on native desktop blur and rejects stale dismissals", async () => {
  const fixture = setup();
  await settle();
  fixture.controller.toggle("menu");
  const first = fixture.calls.at(-1)!;
  assert.equal(first.active, true);
  fixture.dismiss(first.revision);
  assert.deepEqual(fixture.values, ["menu", null]);
  assert.equal(fixture.calls.at(-1)?.active, false);
  fixture.controller.toggle("card");
  const reopened = fixture.calls.at(-1)!;
  assert.ok(reopened.revision > first.revision);
  fixture.dismiss(first.revision);
  assert.equal(fixture.values.at(-1), "card");
  fixture.window.dispatchEvent(new Event("blur"));
  assert.equal(fixture.values.at(-1), null);
  assert.equal(fixture.calls.at(-1)?.active, false);
  fixture.controller.dispose();
  await settle();
  assert.equal(fixture.nativeListeners(), 0);
  assert.equal(fixture.window.listeners + fixture.document.listeners, 0);
});

test("production Escape, hide, suspend and disposal revoke focus without delayed reopen", async () => {
  const pending: ((value: { active: boolean; revision: number }) => void)[] =
    [];
  const requests: { active: boolean; revision: number }[] = [];
  const fixture = setup({
    interaction: (value) => {
      requests.push(value);
      return new Promise((resolve) => pending.push(resolve));
    },
  });
  await settle();
  const triggers = [
    () => {
      const event = new Event("keydown", { cancelable: true });
      Object.defineProperty(event, "key", { value: "Escape" });
      fixture.window.dispatchEvent(event);
      assert.equal(event.defaultPrevented, true);
    },
    () =>
      fixture.window.dispatchEvent(
        new CustomEvent("view-visibility", { detail: { visible: false } }),
      ),
    () =>
      fixture.window.dispatchEvent(
        new CustomEvent("pet-visibility", { detail: { visible: false } }),
      ),
    () =>
      fixture.window.dispatchEvent(
        new CustomEvent("desktop-runtime-state", { detail: { paused: true } }),
      ),
    () => {
      fixture.document.hidden = true;
      fixture.document.dispatchEvent(new Event("visibilitychange"));
    },
    () => fixture.controller.close(),
  ];
  for (const trigger of triggers) {
    fixture.document.hidden = false;
    fixture.controller.toggle("menu");
    trigger();
    assert.equal(fixture.values.at(-1), null);
    assert.equal(requests.at(-1)?.active, false);
  }
  fixture.controller.toggle("card");
  const countBeforeDispose = fixture.values.length;
  fixture.controller.dispose();
  assert.equal(requests.at(-1)?.active, false);
  assert.ok(
    requests.every(
      (value, index) =>
        index === 0 || value.revision > requests[index - 1].revision,
    ),
  );
  for (let index = pending.length - 1; index >= 0; index--)
    pending[index](requests[index]);
  await settle();
  fixture.window.dispatchEvent(new Event("blur"));
  fixture.dismiss(requests.at(-1)?.revision);
  fixture.controller.toggle("menu");
  assert.equal(fixture.values.length, countBeforeDispose);
  assert.equal(fixture.window.listeners + fixture.document.listeners, 0);
  assert.equal(fixture.nativeListeners(), 0);
});

test("production late native subscription is removed after unmount, and focus failure closes the overlay", async () => {
  let finish: ((remove: () => void) => void) | undefined;
  let removed = 0;
  const late = setup({
    listenDismiss: () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  });
  late.controller.toggle("menu");
  late.controller.dispose();
  finish?.(() => {
    removed++;
  });
  await settle();
  assert.equal(removed, 1);
  const failed = setup({
    interaction: async (value) => {
      if (value.active) throw Error("focus denied");
      return value;
    },
  });
  failed.controller.toggle("menu");
  await settle();
  assert.equal(failed.values.at(-1), null);
  assert.equal(failed.errors.length, 1);
  failed.controller.dispose();
});
