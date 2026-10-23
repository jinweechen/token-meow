import {
  AmbientLight, Box3, DirectionalLight, LoadingManager, PerspectiveCamera, Scene,
  SRGBColorSpace, Vector3, WebGLRenderer,
  type BufferGeometry, type Material, type Object3D, type Skeleton, type Texture,
} from "three";
import { GLTFLoader, type GLTFParser } from "three/addons/loaders/GLTFLoader.js";
import { VRMLoaderPlugin, VRMMetaLoaderPlugin, VRMUtils, type VRM } from "@pixiv/three-vrm";
import type { PetState } from "../../shared/contracts";
import { validPetPhysicsContact, type PetPhysicsPaint, type PetPhysicsPaintTicket } from "../../shared/petPhysics";
import { validateVrmModel } from "./vrmModelValidation";

export interface VrmRenderIntent {
  action: PetState;
  reaction: string | null;
  reducedMotion: boolean;
  paused: boolean;
  visible: boolean;
  physicsTicket?: PetPhysicsPaintTicket | null;
}
interface RendererCallbacks { onReady: () => void; onFailure: (message: string) => void; onPainted?: (paint: PetPhysicsPaint) => void }
const FRAME_MS = 1000 / 24;
const EXPRESSION_NAMES = ["happy", "relaxed", "surprised", "sad", "angry", "blink", "blinkLeft", "blinkRight"];

// 解析失败或取消时，部分 dependency 仍可能成功。逐项登记保证这些迟到资源也会被释放。
class OwnedResources {
  private roots = new Set<Object3D>();
  private textures = new Set<Texture>();
  private materials = new Set<Material>();
  private geometries = new Set<BufferGeometry>();
  private skeletons = new Set<Skeleton>();
  private disposed = new WeakSet<object>();
  private closedImages = new WeakSet<object>();
  private ended = false;

  capture(value: unknown): void {
    if (Array.isArray(value)) { value.forEach((item) => this.capture(item)); return; }
    if (!value || typeof value !== "object") return;
    const resource = value as { isObject3D?: boolean; isTexture?: boolean; isMaterial?: boolean; isBufferGeometry?: boolean };
    if (resource.isObject3D) this.roots.add(value as Object3D);
    else if (resource.isTexture) this.textures.add(value as Texture);
    else if (resource.isMaterial) this.materials.add(value as Material);
    else if (resource.isBufferGeometry) this.geometries.add(value as BufferGeometry);
    if (this.ended) this.sweep();
  }

  observe(parser: GLTFParser): void {
    const dependency = parser.getDependency.bind(parser);
    parser.getDependency = (type, index) => dependency(type, index).then((value: unknown) => { this.capture(value); return value; });
    const image = parser.loadImageSource.bind(parser);
    parser.loadImageSource = (index, loader) => image(index, loader).then((value) => { this.capture(value); return value; });
    const geometries = parser.loadGeometries.bind(parser);
    parser.loadGeometries = (primitives) => geometries(primitives).then((value) => { this.capture(value); return value; });
  }

  private collectMaterial(material: Material) {
    this.materials.add(material);
    const uniforms = (material as Material & { uniforms?: Record<string, { value: unknown }> }).uniforms;
    const candidates: unknown[] = [...Object.values(material), ...Object.values(uniforms ?? {}).map((uniform) => uniform.value)];
    for (const candidate of candidates) {
      if (candidate && typeof candidate === "object" && (candidate as Texture).isTexture) this.textures.add(candidate as Texture);
    }
  }

  private sweep() {
    for (const root of this.roots) root.traverse((object) => {
      const renderable = object as Object3D & { geometry?: BufferGeometry; material?: Material | Material[]; skeleton?: Skeleton };
      if (renderable.geometry) this.geometries.add(renderable.geometry);
      if (renderable.skeleton) this.skeletons.add(renderable.skeleton);
      if (renderable.material) for (const material of Array.isArray(renderable.material) ? renderable.material : [renderable.material]) this.collectMaterial(material);
    });
    for (const material of this.materials) this.collectMaterial(material);
    // deepDispose 是官方模型树释放入口；独立登记的资源覆盖解析失败留下的半成品。
    for (const root of this.roots) {
      if (this.disposed.has(root)) continue;
      const resources: object[] = [];
      root.traverse((object) => {
        const renderable = object as Object3D & { geometry?: BufferGeometry; material?: Material | Material[]; skeleton?: Skeleton };
        if (renderable.geometry) resources.push(renderable.geometry);
        if (renderable.skeleton) resources.push(renderable.skeleton);
        if (renderable.material) resources.push(...(Array.isArray(renderable.material) ? renderable.material : [renderable.material]));
      });
      VRMUtils.deepDispose(root);
      this.disposed.add(root);
      resources.forEach((resource) => this.disposed.add(resource));
    }
    for (const texture of this.textures) {
      if (!this.disposed.has(texture)) { texture.dispose(); this.disposed.add(texture); }
      const images: unknown[] = [texture.source?.data, texture.image];
      for (const image of images.flat()) {
        if (typeof ImageBitmap !== "undefined" && image instanceof ImageBitmap && !this.closedImages.has(image)) {
          image.close(); this.closedImages.add(image);
        }
      }
    }
    for (const resource of [...this.materials, ...this.geometries, ...this.skeletons]) {
      if (!this.disposed.has(resource)) { resource.dispose(); this.disposed.add(resource); }
    }
  }

  dispose() { this.ended = true; this.sweep(); }
}

export class VrmRenderer {
  private renderer: WebGLRenderer | null = null;
  private readonly scene = new Scene();
  private readonly camera = new PerspectiveCamera(30, 1, 0.01, 100);
  private readonly resources = new OwnedResources();
  private vrm: VRM | null = null;
  private observer: ResizeObserver | null = null;
  private raf: number | null = null;
  private ended = false;
  private failed = false;
  private firstFrame = false;
  private urls = new Set<string>();
  private intent: VrmRenderIntent = { action: "idle", reaction: null, reducedMotion: false, paused: false, visible: true };
  private time = 0;
  private reactionTime = 0;
  private previousFrame: number | null = null;
  private previousUpdate: number | null = null;
  private frameBox: Box3 | null = null;
  private parser: GLTFParser | null = null;
  private viewport: { width: number; height: number; ratio: number } | null = null;
  private standingFeet: Vector3[] = [];

  constructor(private readonly canvas: HTMLCanvasElement, private readonly callbacks: RendererCallbacks) {
    canvas.addEventListener("webglcontextlost", this.contextLost);
  }

  private contextLost = (event: Event) => {
    event.preventDefault();
    if (!this.ended) this.fail("3D 渲染上下文已丢失，请重新预览模型");
  };

  private fail(message: string) {
    if (this.ended || this.failed) return;
    this.failed = true;
    this.stop();
    this.callbacks.onFailure(message);
    this.dispose();
  }

  async load(buffer: ArrayBuffer, expectedBytes: number): Promise<void> {
    try {
      const validated = validateVrmModel(buffer, expectedBytes);
      if (this.ended) return;
      const context = this.canvas.getContext("webgl2", { alpha: true, antialias: true, premultipliedAlpha: true, powerPreference: "low-power" });
      if (!context) throw new Error("当前环境无法创建 WebGL2");
      this.renderer = new WebGLRenderer({ canvas: this.canvas, context, alpha: true, antialias: true });
      this.renderer.debug.onShaderError = () => { throw new Error("模型材质无法在当前 WebGL2 环境编译"); };
      this.renderer.setClearColor(0x000000, 0);
      this.renderer.outputColorSpace = SRGBColorSpace;
      this.scene.add(new AmbientLight(0xffffff, 1.4));
      const light = new DirectionalLight(0xffffff, 2.4);
      light.position.set(1, 2, 3); this.scene.add(light);
      const imageUrls = validated.images.map((image) => {
        const url = URL.createObjectURL(new Blob([new Uint8Array(buffer, image.offset, image.length)], { type: "image/png" }));
        this.urls.add(url); return url;
      });
      let resourceFailed = false;
      const manager = new LoadingManager();
      manager.setURLModifier((url) => {
        if (!this.urls.has(url) || this.ended) throw new Error("VRM 请求了未登记或已取消的资源");
        return url;
      });
      manager.onError = () => { resourceFailed = true; };
      const loader = new GLTFLoader(manager);
      loader.register((parser) => {
        this.parser = parser;
        this.resources.observe(parser);
        // 只改本次 parser 的内存副本。所有 URI 都来自本实例已检查的嵌入图片。
        parser.json.images = parser.json.images?.map((image: Record<string, unknown>, index: number) => ({ name: image.name, mimeType: "image/png", uri: imageUrls[index] }));
        // 本期使用规范 humanoid/expressions 驱动状态，不解析、执行包内动画轨道。
        parser.json.animations = [];
        return new VRMLoaderPlugin(parser, { metaPlugin: new VRMMetaLoaderPlugin(parser, { needThumbnailImage: false, acceptV0Meta: false }) });
      });
      const gltf = await loader.parseAsync(buffer, "");
      this.resources.capture(gltf.scenes);
      for (const resource of gltf.parser.associations.keys()) this.resources.capture(resource);
      if (this.ended) { this.resources.dispose(); return; }
      if (resourceFailed) throw new Error("模型图片解码失败，请检查内嵌 PNG");
      const vrm = gltf.userData.vrm as VRM | undefined;
      if (!vrm || vrm.meta.metaVersion !== "1") throw new Error("模型缺少可用的 VRM 1.0 humanoid 信息");
      this.vrm = vrm;
      this.scene.add(vrm.scene);
      vrm.scene.traverse((object) => { object.frustumCulled = false; });
      this.pose(0, true);
      this.frameBox = new Box3().setFromObject(vrm.scene, true);
      this.standingFeet = ["leftFoot", "rightFoot"]
        .map(name => vrm.humanoid.getRawBoneNode(name as "leftFoot" | "rightFoot"))
        .filter((node): node is Object3D => !!node).map(node => node.getWorldPosition(new Vector3()));
      const dimensions = this.frameBox.getSize(new Vector3());
      if (this.frameBox.isEmpty() || ![dimensions.x, dimensions.y, dimensions.z].every((value) => Number.isFinite(value) && value <= 10_000) || dimensions.y <= 0.001) throw new Error("模型尺寸无法正确显示");
      this.observer = new ResizeObserver(() => this.resize());
      this.observer.observe(this.canvas);
      this.resize(false);
      this.synchronize();
    } catch (error) {
      if (this.parser) for (const resource of this.parser.associations.keys()) this.resources.capture(resource);
      this.resources.dispose();
      if (!this.ended) this.fail(error instanceof Error ? error.message : "VRM 模型加载失败");
    } finally {
      this.revokeUrls();
    }
  }

  setIntent(intent: VrmRenderIntent) {
    if (this.ended) return;
    if (intent.reaction !== this.intent.reaction) this.reactionTime = 0;
    this.intent = intent;
    this.synchronize();
  }

  private synchronize() {
    this.stop();
    if (!this.vrm || this.ended || !this.intent.visible) return;
    this.draw(0);
    if (!this.ended && !this.intent.paused && !this.intent.reducedMotion) this.raf = requestAnimationFrame(this.tick);
  }

  private stop() {
    if (this.raf !== null) cancelAnimationFrame(this.raf);
    this.raf = null;
    // 恢复时从零 delta 开始；隐藏或减少动画期间没有需要补播的时间债。
    this.previousFrame = null;
    this.previousUpdate = null;
  }

  private tick = (now: number) => {
    this.raf = null;
    if (this.ended || this.intent.paused || this.intent.reducedMotion || !this.intent.visible) return;
    const elapsed = this.previousFrame === null ? FRAME_MS : now - this.previousFrame;
    if (elapsed >= FRAME_MS - 0.25) {
      const delta = this.previousUpdate === null ? 0 : Math.min(0.1, Math.max(0, (now - this.previousUpdate) / 1000));
      this.previousUpdate = now;
      this.previousFrame = now - (elapsed % FRAME_MS);
      this.draw(delta);
    }
    if (!this.ended) this.raf = requestAnimationFrame(this.tick);
  };

  private pose(delta: number, standing = false) {
    const vrm = this.vrm;
    if (!vrm) return;
    this.time += delta;
    this.reactionTime += delta;
    const staticFrame = this.intent.paused || this.intent.reducedMotion;
    const breathe = staticFrame ? 0 : Math.sin(this.time * 1.6);
    vrm.humanoid.resetNormalizedPose();
    const bone = (name: Parameters<typeof vrm.humanoid.getNormalizedBoneNode>[0], x = 0, y = 0, z = 0) => {
      vrm.humanoid.getNormalizedBoneNode(name)?.rotation.set(x, y, z);
    };
    bone("leftUpperArm", 0, 0, -1.12);
    bone("rightUpperArm", 0, 0, 1.12);
    bone("leftLowerArm", -0.12); bone("rightLowerArm", -0.12);
    bone("spine", breathe * 0.012);
    bone("head", 0, staticFrame ? 0 : Math.sin(this.time * 0.55) * 0.025);
    const manager = vrm.expressionManager;
    const expression = (name: string, value: number) => {
      if (manager?.getExpression(name)) manager.setValue(name, Math.min(1, Math.max(0, value)));
    };
    EXPRESSION_NAMES.forEach((name) => expression(name, 0));
    const blinkCycle = this.time % 5.4;
    expression("blink", !staticFrame && blinkCycle > 5.12 ? Math.sin((blinkCycle - 5.12) / 0.28 * Math.PI) : 0);
    switch (this.intent.action) {
      case "active": bone("head", 0.1); bone("leftLowerArm", -0.3); bone("rightLowerArm", -0.3); break;
      case "complete": expression("happy", 0.65); bone("head", 0, 0, 0.035); bone("rightUpperArm", -0.1, 0, 0.82); break;
      case "alert": expression("surprised", 0.5); bone("head", -0.05, 0, -0.055); break;
      case "sleep": expression("blink", 1); expression("relaxed", 0.4); bone("head", 0.24); break;
    }
    const amount = staticFrame ? 0.85 : Math.max(0, Math.sin(Math.min(1, this.reactionTime / 1.8) * Math.PI));
    if (this.intent.reaction === "affection" || this.intent.reaction === "happy") {
      expression("happy", Math.max(this.intent.action === "complete" ? 0.65 : 0, amount * 0.85));
      expression("relaxed", amount * 0.3); bone("head", 0.05 * amount, 0, 0.1 * amount);
    } else if (this.intent.reaction === "wink") {
      expression("blinkLeft", amount); expression("happy", amount * 0.5); bone("head", 0, 0, -0.07 * amount);
    } else if (this.intent.reaction === "stretch") {
      bone("leftUpperArm", 0, 0, -1.12 + amount * 0.55); bone("rightUpperArm", 0, 0, 1.12 - amount * 0.55);
      expression("relaxed", amount * 0.45);
    }
    // 攀爬只改变标准人形骨骼，窗口位置和运动许可始终由原生规划器持有。
    const physics = this.intent.physicsTicket;
    if (!standing && physics?.kind === "climb" && !staticFrame && this.intent.action === "idle" && !this.intent.reaction) {
      const direction = physics.direction === "down" ? -1 : 1;
      const step = Math.sin(this.time * 4.5 * direction);
      bone("leftUpperArm", -0.2, 0, 0.85 + step * 0.18);
      bone("rightUpperArm", -0.2, 0, -0.85 + step * 0.18);
      bone("leftLowerArm", -0.8); bone("rightLowerArm", -0.8);
      bone("leftUpperLeg", -0.3 - step * 0.18); bone("rightUpperLeg", -0.3 + step * 0.18);
      bone("leftLowerLeg", 0.45 + step * 0.2); bone("rightLowerLeg", 0.45 - step * 0.2);
      bone("spine", 0, 0, physics.side === "right" ? -0.06 : 0.06);
    }
    vrm.update(delta);
  }

  private draw(delta: number) {
    if (!this.renderer || !this.vrm || this.ended || !this.intent.visible) return;
    try {
      this.pose(delta);
      this.renderer.render(this.scene, this.camera);
      if (this.renderer.getContext().isContextLost()) throw new Error("3D 渲染上下文已丢失");
      if (!this.firstFrame) { this.firstFrame = true; this.callbacks.onReady(); }
      const contact = this.contact();
      if (contact) this.callbacks.onPainted?.({ contact, ticket: this.intent.physicsTicket ?? null,
        climbReady: this.standingFeet.length === 2 && !this.intent.paused && !this.intent.reducedMotion
          && this.intent.action === "idle" && !this.intent.reaction });
    } catch (error) {
      this.fail(error instanceof Error ? error.message : "VRM 首帧绘制失败");
    }
  }

  private contact() {
    const box = this.frameBox;
    if (!box || this.standingFeet.length !== 2) return null;
    const canvas = this.canvas.getBoundingClientRect(), vw = window.innerWidth, vh = window.innerHeight;
    if (canvas.width < 1 || canvas.height < 1) return null;
    const project = (point: Vector3) => {
      const projected = point.clone().project(this.camera);
      return { x: canvas.left + (projected.x + 1) * canvas.width / 2, y: canvas.top + (1 - projected.y) * canvas.height / 2 };
    };
    const corners = [box.min.x, box.max.x].flatMap(x => [box.min.y, box.max.y].flatMap(y => [box.min.z, box.max.z].map(z => project(new Vector3(x, y, z)))));
    // 同一模型/视口保持站姿支点；脚踝到鞋底的模型留白也纳入投影，不能逐帧改变原生接触偏移。
    const feet = this.standingFeet.map(foot => project(new Vector3(foot.x, Math.min(foot.y, box.min.y), foot.z)));
    const contact = { viewportWidth: vw, viewportHeight: vh,
      leftInset: Math.max(0, Math.min(vw, Math.min(...corners.map(point => point.x)))),
      rightInset: Math.max(0, vw - Math.max(0, Math.min(vw, Math.max(...corners.map(point => point.x))))),
      bottomInset: Math.max(0, vh - Math.max(0, Math.min(vh, Math.max(...feet.map(point => point.y))))) };
    return validPetPhysicsContact(contact) ? contact : null;
  }

  private resize(paint = true) {
    if (!this.renderer || !this.frameBox || this.ended) return;
    const width = Math.max(1, this.canvas.clientWidth || 180), height = Math.max(1, this.canvas.clientHeight || 240);
    const ratio = Math.min(window.devicePixelRatio || 1, 1.5);
    if (this.viewport?.width === width && this.viewport.height === height && this.viewport.ratio === ratio) return;
    this.viewport = { width, height, ratio };
    this.renderer.setPixelRatio(ratio);
    this.renderer.setSize(width, height, false);
    this.camera.aspect = width / height;
    const size = this.frameBox.getSize(new Vector3()), center = this.frameBox.getCenter(new Vector3());
    const tangent = Math.tan(this.camera.fov * Math.PI / 360);
    const distance = Math.max(size.y / (2 * tangent), size.x / (2 * tangent * this.camera.aspect)) * 1.18 + size.z / 2;
    this.camera.near = Math.max(0.001, distance / 100);
    this.camera.far = Math.max(20, distance * 10);
    this.camera.position.set(center.x, center.y, center.z + distance);
    this.camera.lookAt(center);
    this.camera.updateProjectionMatrix();
    if (paint) this.draw(0);
  }

  private revokeUrls() { for (const url of this.urls) URL.revokeObjectURL(url); this.urls.clear(); }

  dispose() {
    if (this.ended) return;
    this.ended = true;
    this.stop();
    this.observer?.disconnect(); this.observer = null;
    this.canvas.removeEventListener("webglcontextlost", this.contextLost);
    this.revokeUrls();
    this.resources.dispose();
    this.vrm = null;
    this.scene.clear();
    // React StrictMode 和同一 canvas 上的模型切换会重新创建 renderer；主动 loseContext
    // 会让下一位 owner 拿到已丢失的上下文。dispose 释放实例资源，真实 contextlost 另行降级。
    this.renderer?.resetState();
    this.renderer?.dispose();
    this.renderer = null;
  }
}
