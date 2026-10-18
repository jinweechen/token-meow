import assert from "node:assert/strict";
import test from "node:test";
import {
  isUsageChartType,
  usageTrendGeometry,
  usageTrendValue,
} from "./usageTrendGeometry";

const plot = { left: 20, top: 10, width: 120, height: 100 };

test("trend values keep explicit zero separate from missing or invalid costs", () => {
  for (const value of [
    null,
    undefined,
    "",
    " ",
    "bad",
    "NaN",
    "Infinity",
    "0x10",
    "1e3",
    -1,
    NaN,
    Infinity,
  ]) {
    assert.equal(usageTrendValue(value), null, String(value));
  }
  assert.equal(usageTrendValue("0"), 0);
  assert.equal(usageTrendValue(0), 0);
  assert.equal(usageTrendValue("0.00000001"), 0.00000001);
});

test("unknown time buckets split paths without compressing their time positions", () => {
  const geometry = usageTrendGeometry([2, null, 0, 4, null, 1], plot);
  assert.deepEqual(
    geometry.segments.map((segment) => segment.map((point) => point.index)),
    [[0], [2, 3], [5]],
  );
  assert.deepEqual(
    geometry.segments.map((segment) => segment.map((point) => point.x)),
    [[30], [70, 90], [130]],
  );
  assert.equal(geometry.coordinates[1], null);
  assert.equal(geometry.coordinates[2]!.y, 110);
  assert.equal(geometry.coordinates[3]!.y, 10);
  assert.equal(geometry.max, 4);
});

test("zero-only and singleton plots retain visible finite baseline coordinates", () => {
  const zero = usageTrendGeometry([0, 0], plot);
  assert.equal(zero.max, 1);
  assert.deepEqual(
    zero.segments[0].map((point) => point.y),
    [110, 110],
  );
  const singleton = usageTrendGeometry([3], plot);
  assert.equal(singleton.segments[0].length, 1);
  assert.deepEqual(singleton.coordinates[0], {
    index: 0,
    value: 3,
    x: 80,
    y: 10,
  });
  const missing = usageTrendGeometry([null, NaN, -2], plot);
  assert.deepEqual(missing.segments, []);
  assert.ok(Number.isFinite(missing.step));
});

test("saved chart preference accepts only supported chart types", () => {
  for (const type of ["bar", "line", "area"])
    assert.equal(isUsageChartType(type), true);
  for (const type of [null, "", "pie", "LINE", {}, 0])
    assert.equal(isUsageChartType(type), false);
});
