import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { HD_LOGICAL_POSES, hdPoseFrame } from "../features/pet/hdPoseLayout";
import type { BuiltinAnimationManifest } from "../features/pet/builtinAnimationSkin";

async function fixture() {
  return JSON.parse(await readFile(new URL("../../assets/pets/nekomi/manifest-animation-v12.json", import.meta.url), "utf8")) as BuiltinAnimationManifest;
}

test("current animation maps work states and all seven cute poses to their logical frames", async () => {
  const manifest = await fixture();
  assert.equal(manifest.renderer_kind, "builtin-animation-clips");
  assert.equal(manifest.skin_id, "nekomi-animation-v12");
  for (const action of ["idle", "active", "complete", "alert", "sleep"] as const) {
    for (const phase of ["start", "loop", "end"]) {
      const frame = hdPoseFrame(manifest, action, phase, false, false);
      assert.equal(HD_LOGICAL_POSES[frame.frame], action);
    }
    assert.equal(HD_LOGICAL_POSES[hdPoseFrame(manifest, action, "loop", true, false).frame], "idle");
  }
  assert.equal(HD_LOGICAL_POSES[hdPoseFrame(manifest, "complete", "loop", false, true).frame], "sleep");
  assert.equal(HD_LOGICAL_POSES[hdPoseFrame(manifest, "complete", "static", false, true).frame], "sleep");
  const names = manifest.variants.tap.map((frame) => HD_LOGICAL_POSES[frame]);
  assert.deepEqual(new Set(names), new Set(["shy", "affection", "stretch", "wink", "happy", "bow", "hug-tail"]));
  assert.deepEqual(new Set(manifest.variants.idle.map((frame) => HD_LOGICAL_POSES[frame])),
    new Set([...names, "sleep", "walk-right", "walk-left", "walk-left-to-right", "walk-right-to-left"]));
});

