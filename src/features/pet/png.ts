const MAX_BYTES = 4 * 1024 * 1024;
const MAX_WIDTH = 1024, MAX_HEIGHT = 1536;

export function transparentPadding(corners: readonly number[]) {
  return corners.length === 4 && corners.every((alpha) => alpha === 0);
}

/** Inspect an HD original before decoding; each RGBA image fits one 6 MiB slot. */
export function inspectPng(bytes: Uint8Array) {
  const signature = [137, 80, 78, 71, 13, 10, 26, 10];
  if (bytes.length > MAX_BYTES) throw Error("角色 PNG 文件超限");
  if (bytes.length < 45 || !signature.every((byte, index) => bytes[index] === byte))
    throw Error("角色 PNG 标记无效");
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const type = (offset: number) => String.fromCharCode(...bytes.subarray(offset, offset + 4));
  if (view.getUint32(8) !== 13 || type(12) !== "IHDR") throw Error("角色 PNG 头无效");
  const width = view.getUint32(16), height = view.getUint32(20);
  if (!width || !height || width > MAX_WIDTH || height > MAX_HEIGHT ||
      width * height * 4 > MAX_WIDTH * MAX_HEIGHT * 4)
    throw Error("角色 PNG 解码尺寸超限");
  if (bytes[24] !== 8 || bytes[25] !== 6 || bytes[26] !== 0 ||
      bytes[27] !== 0 || bytes[28] > 1)
    throw Error("角色 PNG 必须是带 alpha 的 8 位 RGBA 静态图");
  let offset = 8, idat = false, ended = false;
  while (offset + 12 <= bytes.length) {
    const length = view.getUint32(offset), chunk = type(offset + 4);
    if (length > bytes.length - offset - 12) throw Error("角色 PNG 数据截断");
    if (["acTL", "fcTL", "fdAT"].includes(chunk)) throw Error("角色 PNG 不接受内嵌动画");
    if (chunk === "IHDR" && offset !== 8) throw Error("角色 PNG 头重复");
    if (chunk === "IDAT") idat = true;
    offset += length + 12;
    if (chunk === "IEND") {
      ended = length === 0 && offset === bytes.length;
      break;
    }
  }
  if (!ended || !idat) throw Error("角色 PNG 数据不完整");
  return { width, height };
}
