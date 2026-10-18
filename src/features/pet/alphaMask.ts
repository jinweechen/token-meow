/** Conservative alpha cells, not an exact per-pixel native hit-test. */
export const MAX_ALPHA_GRID_WIDTH = 128;
export const MAX_ALPHA_GRID_HEIGHT = 192;
export const MAX_ALPHA_MASK_BYTES = 3072;
export const ALPHA_MASK_RENEW_MS = 400;
export const ALPHA_MASK_LEASE_MS = 1000;
const MAX_CLIENT_SIDE = 8192;

export interface AlphaMaskGrid {
  gridWidth: number;
  gridHeight: number;
  /** Row-major, least significant bit first. Suitable for the native Vec<u8>. */
  bits: number[];
}

/**
 * The caller renders the current frame AND its exact transforms/clipping onto
 * a full physical-client-size canvas first. Do not pass original PNG pixels:
 * those do not describe the current window layout, device scale, or UI.
 *
 * Every source alpha > 0 preserves its whole grid cell. Native code adds a
 * physical-pixel edge margin and unions separately supplied protected UI rects.
 * Missing/unreadable canvas pixels must invalidate the previous native mask.
 */
export function buildConservativeAlphaMask(
  rgba: Uint8ClampedArray | Uint8Array,
  clientWidth: number,
  clientHeight: number,
): AlphaMaskGrid {
  if (
    !Number.isSafeInteger(clientWidth) || !Number.isSafeInteger(clientHeight) ||
    clientWidth <= 0 || clientHeight <= 0 ||
    clientWidth > MAX_CLIENT_SIDE || clientHeight > MAX_CLIENT_SIDE ||
    rgba.length !== clientWidth * clientHeight * 4
  ) {
    throw new RangeError("Alpha mask requires exact physical client RGBA pixels");
  }
  const gridWidth = Math.min(clientWidth, MAX_ALPHA_GRID_WIDTH);
  const gridHeight = Math.min(clientHeight, MAX_ALPHA_GRID_HEIGHT);
  const bits = new Array<number>(Math.ceil(gridWidth * gridHeight / 8)).fill(0);
  for (let y = 0; y < clientHeight; y += 1) {
    const gridY = Math.floor(y * gridHeight / clientHeight);
    for (let x = 0; x < clientWidth; x += 1) {
      if (rgba[(y * clientWidth + x) * 4 + 3] === 0) continue;
      const gridX = Math.floor(x * gridWidth / clientWidth);
      const bit = gridY * gridWidth + gridX;
      bits[bit >>> 3] |= 1 << (bit & 7);
    }
  }
  return { gridWidth, gridHeight, bits };
}
