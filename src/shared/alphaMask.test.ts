import assert from "node:assert/strict";
import test from "node:test";
import {
  ALPHA_MASK_LEASE_MS,
  ALPHA_MASK_RENEW_MS,
  MAX_ALPHA_MASK_BYTES,
  buildConservativeAlphaMask,
} from "../features/pet/alphaMask";

const isSet = (bits: number[], width: number, x: number, y: number) => {
  const index = y * width + x;
  return (bits[index >>> 3] & (1 << (index & 7))) !== 0;
};

test("alpha grid preserves alpha=1 and packs row-major low bits first", () => {
  const pixels = new Uint8ClampedArray(3 * 2 * 4);
  pixels[3] = 1;
  pixels[(1 * 3 + 2) * 4 + 3] = 255;
  const grid = buildConservativeAlphaMask(pixels, 3, 2);
  assert.deepEqual(grid, { gridWidth: 3, gridHeight: 2, bits: [0b100001] });
  assert.equal(pixels[3], 1);
});

test("max pooling retains every opaque source pixel including uneven cell boundaries", () => {
  const width = 259;
  const height = 389;
  const pixels = new Uint8Array(width * height * 4);
  const points = [[0, 0], [1, 1], [2, 2], [128, 192], [257, 387], [258, 388]];
  for (const [x, y] of points) pixels[(y * width + x) * 4 + 3] = 1;
  const grid = buildConservativeAlphaMask(pixels, width, height);
  assert.equal(grid.bits.length, MAX_ALPHA_MASK_BYTES);
  assert.equal(grid.gridWidth, 128);
  assert.equal(grid.gridHeight, 192);
  for (const [x, y] of points) {
    assert.equal(isSet(grid.bits, 128, Math.floor(x * 128 / width), Math.floor(y * 192 / height)), true);
  }
  assert.equal(isSet(grid.bits, 128, 64, 0), false);
});

test("transparent RGB does not create a hit and odd-size padding bits stay zero", () => {
  const pixels = new Uint8Array(9 * 4);
  pixels.fill(255);
  for (let i = 3; i < pixels.length; i += 4) pixels[i] = 0;
  assert.deepEqual(buildConservativeAlphaMask(pixels, 9, 1).bits, [0, 0]);
  pixels[8 * 4 + 3] = 255;
  assert.deepEqual(buildConservativeAlphaMask(pixels, 9, 1).bits, [0, 1]);
});

test("invalid geometry or mismatched bytes is rejected without inventing a mask", () => {
  for (const [width, height] of [[0, 1], [-1, 1], [1.5, 1], [1, NaN], [8193, 1], [Infinity, 1]]) {
    assert.throws(() => buildConservativeAlphaMask(new Uint8Array(), width, height), RangeError);
  }
  assert.throws(() => buildConservativeAlphaMask(new Uint8Array(3), 1, 1), RangeError);
  assert.throws(() => buildConservativeAlphaMask(new Uint8Array(8), 1, 1), RangeError);
});

test("fully opaque client stays bounded and lease has multiple renewal opportunities", () => {
  const pixels = new Uint8Array(512 * 768 * 4).fill(255);
  const grid = buildConservativeAlphaMask(pixels, 512, 768);
  assert.equal(grid.bits.length, MAX_ALPHA_MASK_BYTES);
  assert.equal(grid.bits.every((value) => value === 255), true);
  assert.ok(ALPHA_MASK_RENEW_MS * 2 < ALPHA_MASK_LEASE_MS);
});
