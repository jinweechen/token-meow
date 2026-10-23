// 只允许本期 renderer 能预算和离线解释的单文件 VRM1，校验在任何解码或 WebGL 分配前执行。
const LIMITS = Object.freeze({
  encoded: 64 * 1024 * 1024, json: 4 * 1024 * 1024, pixels: 96 * 1024 * 1024,
  accessorBytes: 128 * 1024 * 1024, side: 4096, nodes: 512, joints: 200,
  triangles: 100_000, accessors: 4096, primitives: 256, resources: 128, morphs: 64,
});
const EXTENSIONS = new Set([
  "VRMC_vrm", "VRMC_springBone", "VRMC_node_constraint", "VRMC_materials_mtoon",
  "VRMC_materials_hdr_emissiveMultiplier", "KHR_materials_unlit", "KHR_texture_transform",
]);
type JsonObject = Record<string, unknown>;
export interface ValidatedVrm {
  images: { offset: number; length: number }[];
}
function requireValue(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`VRM 文件不受支持：${message}`);
}
function object(value: unknown): JsonObject {
  requireValue(value !== null && typeof value === "object" && !Array.isArray(value), "对象结构无效");
  return value as JsonObject;
}
function list(value: unknown, max = 100_000): unknown[] {
  requireValue(value === undefined || Array.isArray(value), "列表结构无效");
  const result = (value ?? []) as unknown[];
  requireValue(result.length <= max, "列表超过上限");
  return result;
}
function integer(value: unknown, max: number, min = 0): number {
  requireValue(typeof value === "number" && Number.isSafeInteger(value) && value >= min && value <= max, "整数或资源预算无效");
  return value;
}
function reference(value: unknown, entries: unknown[]): number {
  return integer(value, entries.length - 1);
}
function range(offset: number, size: number, limit: number) {
  requireValue(Number.isSafeInteger(offset + size) && offset + size <= limit, "二进制资源越界");
}

export function validateVrmModel(buffer: ArrayBuffer, expectedBytes?: number): ValidatedVrm {
  requireValue(buffer instanceof ArrayBuffer && buffer.byteLength >= 28 && buffer.byteLength <= LIMITS.encoded, "文件大小无效");
  if (expectedBytes !== undefined) requireValue(buffer.byteLength === expectedBytes, "文件大小与记录不一致");
  const data = new DataView(buffer);
  requireValue(data.getUint32(0, true) === 0x46546c67 && data.getUint32(4, true) === 2 && data.getUint32(8, true) === buffer.byteLength, "需要 GLB 2 文件");
  const jsonSize = integer(data.getUint32(12, true), LIMITS.json, 4);
  requireValue(jsonSize % 4 === 0 && data.getUint32(16, true) === 0x4e4f534a, "JSON 数据块无效");
  range(20, jsonSize + 8, buffer.byteLength);
  const binHeader = 20 + jsonSize;
  const binSize = data.getUint32(binHeader, true), binOffset = binHeader + 8;
  requireValue(binSize % 4 === 0 && data.getUint32(binHeader + 4, true) === 0x004e4942 && binOffset + binSize === buffer.byteLength, "只支持一个内嵌 BIN 数据块");
  const json = object(JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(buffer, 20, jsonSize))));
  requireValue(object(json.asset).version === "2.0", "需要 glTF 2.0");
  requireValue(object(object(json.extensions).VRMC_vrm).specVersion === "1.0", "只支持 VRM 1.0");
  // 不能只检查 extensionsRequired；GLTFLoader 同样会执行 Used 和嵌套扩展。
  const pending: { value: unknown; depth: number }[] = [{ value: json, depth: 0 }];
  let visited = 0;
  while (pending.length) {
    const { value, depth } = pending.pop()!;
    requireValue(++visited <= 200_000 && depth <= 64, "JSON 结构过于复杂");
    if (typeof value === "number") requireValue(Number.isFinite(value), "存在非有限数字");
    if (value === null || typeof value !== "object") continue;
    if (Array.isArray(value)) { for (const child of value) pending.push({ value: child, depth: depth + 1 }); continue; }
    for (const [key, child] of Object.entries(value)) {
      requireValue(key.toLowerCase() !== "uri", "不允许外部、data 或 blob 资源引用");
      if (key === "extensions") for (const extension of Object.keys(object(child))) requireValue(EXTENSIONS.has(extension), `扩展 ${extension}`);
      if (key === "extensionsUsed" || key === "extensionsRequired") {
        for (const extension of list(child, 32)) requireValue(typeof extension === "string" && EXTENSIONS.has(extension), `扩展 ${String(extension)}`);
      }
      pending.push({ value: child, depth: depth + 1 });
    }
  }
  const buffers = list(json.buffers, 1);
  requireValue(buffers.length === 1, "需要单个内嵌 buffer");
  const declaredBin = integer(object(buffers[0]).byteLength, binSize);
  requireValue(binSize - declaredBin <= 3, "BIN 长度不一致");
  const views = list(json.bufferViews, 8192).map((entry): JsonObject & { offset: number; length: number } => {
    const view = object(entry);
    requireValue(view.buffer === 0, "buffer 索引无效");
    const offset = integer(view.byteOffset ?? 0, declaredBin), length = integer(view.byteLength, declaredBin);
    range(offset, length, declaredBin);
    return { ...view, offset, length };
  });
  const images = list(json.images, LIMITS.resources).map((entry) => {
    const image = object(entry);
    requireValue(image.mimeType === "image/png", "只支持内嵌 PNG 图片");
    const view = views[reference(image.bufferView, views)];
    requireValue(view.length >= 45, "PNG 长度无效");
    const start = binOffset + view.offset;
    requireValue(data.getUint32(start) === 0x89504e47 && data.getUint32(start + 4) === 0x0d0a1a0a && data.getUint32(start + 8) === 13 && data.getUint32(start + 12) === 0x49484452, "PNG 头无效");
    const width = integer(data.getUint32(start + 16), LIMITS.side, 1), height = integer(data.getUint32(start + 20), LIMITS.side, 1);
    let cursor = 8, ended = false;
    while (cursor < view.length) {
      range(cursor, 12, view.length);
      const length = data.getUint32(start + cursor), type = data.getUint32(start + cursor + 4);
      range(cursor, length + 12, view.length);
      requireValue(type !== 0x6163544c && type !== 0x6663544c && type !== 0x66644154, "本期不支持 APNG");
      cursor += length + 12;
      if (type === 0x49454e44) { requireValue(length === 0 && cursor === view.length, "PNG 结束块无效"); ended = true; break; }
    }
    requireValue(ended, "PNG 结束块缺失");
    return { offset: start, length: view.length, pixels: width * height * 4 };
  });
  requireValue(images.reduce((sum, image) => sum + image.pixels, 0) <= LIMITS.pixels, "图片基础像素超过 96 MiB");
  const textures = list(json.textures, LIMITS.resources);
  for (const entry of textures) reference(object(entry).source, images);
  list(json.materials, LIMITS.resources);
  const accessors = list(json.accessors, LIMITS.accessors);
  const componentSizes: Record<number, number> = { 5120: 1, 5121: 1, 5122: 2, 5123: 2, 5125: 4, 5126: 4 };
  const elements: Record<string, number> = { SCALAR: 1, VEC2: 2, VEC3: 3, VEC4: 4, MAT2: 4, MAT3: 9, MAT4: 16 };
  let expanded = 0;
  const accessorCounts = accessors.map((entry) => {
    const accessor = object(entry), type = integer(accessor.componentType, 5126), component = componentSizes[type];
    const element = typeof accessor.type === "string" ? elements[accessor.type] : undefined;
    requireValue(component && element, "accessor 类型无效");
    // 小整数矩阵的列对齐与本期无关，明确拒绝避免低估展开字节。
    requireValue(!String(accessor.type).startsWith("MAT") || component === 4, "只支持浮点矩阵 accessor");
    const itemSize = component * element, count = integer(accessor.count, LIMITS.accessorBytes / itemSize, 1);
    expanded += itemSize * count;
    requireValue(expanded <= LIMITS.accessorBytes, "accessor 展开超过 128 MiB");
    if (accessor.bufferView !== undefined) {
      const view = views[reference(accessor.bufferView, views)], offset = integer(accessor.byteOffset ?? 0, view.length);
      const stride = integer(view.byteStride ?? itemSize, 252, itemSize);
      requireValue(stride % component === 0 && (view.offset + offset) % component === 0, "accessor 对齐无效");
      range(offset, (count - 1) * stride + itemSize, view.length);
    }
    if (accessor.sparse !== undefined) {
      const sparse = object(accessor.sparse), sparseCount = integer(sparse.count, count, 1);
      const indices = object(sparse.indices), values = object(sparse.values);
      const indexType = integer(indices.componentType, 5125);
      requireValue(indexType === 5121 || indexType === 5123 || indexType === 5125, "sparse 索引类型无效");
      const indexSize = componentSizes[indexType], indexView = views[reference(indices.bufferView, views)], valueView = views[reference(values.bufferView, views)];
      const indexOffset = integer(indices.byteOffset ?? 0, indexView.length), valueOffset = integer(values.byteOffset ?? 0, valueView.length);
      requireValue(indexView.byteStride === undefined && valueView.byteStride === undefined && (indexView.offset + indexOffset) % indexSize === 0 && (valueView.offset + valueOffset) % component === 0, "sparse 对齐无效");
      range(indexOffset, sparseCount * indexSize, indexView.length); range(valueOffset, sparseCount * itemSize, valueView.length);
      let previous = -1;
      for (let i = 0; i < sparseCount; i++) {
        const at = binOffset + indexView.offset + indexOffset + i * indexSize;
        const value = indexSize === 1 ? data.getUint8(at) : indexSize === 2 ? data.getUint16(at, true) : data.getUint32(at, true);
        requireValue(value > previous && value < count, "sparse 索引无效"); previous = value;
      }
    }
    return count;
  });
  const meshes = list(json.meshes, LIMITS.nodes);
  let primitiveCount = 0;
  const meshTriangles = meshes.map((entry) => list(object(entry).primitives, LIMITS.primitives).reduce<number>((total, value) => {
    requireValue(++primitiveCount <= LIMITS.primitives, "primitive 超过上限");
    const primitive = object(value), attributes = object(primitive.attributes);
    requireValue((primitive.mode ?? 4) === 4, "只支持三角形 primitive");
    const position = reference(attributes.POSITION, accessors), positionAccessor = object(accessors[position]);
    requireValue(positionAccessor.type === "VEC3" && positionAccessor.componentType === 5126, "POSITION 需要浮点 VEC3");
    for (const value of Object.values(attributes)) reference(value, accessors);
    for (const target of list(primitive.targets, LIMITS.morphs)) for (const value of Object.values(object(target))) reference(value, accessors);
    const count = primitive.indices === undefined ? accessorCounts[position] : accessorCounts[reference(primitive.indices, accessors)];
    requireValue(count % 3 === 0, "三角形索引数量无效");
    return total + count / 3;
  }, 0));
  const nodes = list(json.nodes, LIMITS.nodes), parents = new Map<number, number>();
  let renderedTriangles = 0;
  nodes.forEach((entry, index) => {
    const node = object(entry);
    if (node.mesh !== undefined) renderedTriangles += meshTriangles[reference(node.mesh, meshes)];
    for (const value of list(node.children, LIMITS.nodes)) {
      const child = reference(value, nodes);
      requireValue(child !== index && !parents.has(child), "节点关系无效"); parents.set(child, index);
    }
  });
  requireValue(renderedTriangles > 0 && renderedTriangles <= LIMITS.triangles, "渲染三角形超过上限或模型为空");
  nodes.forEach((_, index) => { const seen = new Set<number>(); let current: number | undefined = index; while (current !== undefined) { requireValue(!seen.has(current), "节点存在环"); seen.add(current); current = parents.get(current); } });
  const skins = list(json.skins, LIMITS.nodes), joints = new Set<number>();
  for (const entry of skins) {
    const skin = object(entry), indices = list(skin.joints, LIMITS.joints).map((value) => reference(value, nodes));
    requireValue(new Set(indices).size === indices.length, "skin joint 重复");
    indices.forEach((index) => joints.add(index));
    if (skin.inverseBindMatrices !== undefined) reference(skin.inverseBindMatrices, accessors);
  }
  requireValue(joints.size <= LIMITS.joints, "joint 超过上限");
  const scenes = list(json.scenes, 8);
  requireValue(scenes.length > 0, "缺少场景"); reference(json.scene ?? 0, scenes);
  for (const entry of scenes) for (const node of list(object(entry).nodes, LIMITS.nodes)) reference(node, nodes);
  for (const entry of nodes) if (object(entry).skin !== undefined) reference(object(entry).skin, skins);
  return { images: images.map(({ offset, length }) => ({ offset, length })) };
}
