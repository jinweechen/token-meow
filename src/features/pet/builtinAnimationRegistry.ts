import { builtinAnimationClips, builtinAnimationFrames, builtinAnimationSources } from "./builtinAnimationSkin";
import { AnimationClipRegistry } from "./animationClipRegistry";
import { bindPetWindowLifetime } from "./petWindowLifetime";

let registry: AnimationClipRegistry | null = null;
let terminal = false;
let unbind: (() => void) | null = null;

function release() {
  terminal = true;
  registry?.dispose();
  registry = null;
  unbind = null;
}
function teardown() {
  if (unbind) unbind();
  else release();
}

/** One clip cache per WebView; URL imports alone do not fetch PNG bodies. */
export function getBuiltinAnimationRegistry() {
  if (terminal) throw Error("角色窗口已关闭");
  if (!registry) registry = new AnimationClipRegistry(
    builtinAnimationFrames, builtinAnimationSources, builtinAnimationClips);
  if (!unbind) unbind = bindPetWindowLifetime({ dispose: release }, window);
  return registry;
}

if (import.meta.hot) import.meta.hot.dispose(teardown);
