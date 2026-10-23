import test from "node:test";
import assert from "node:assert/strict";
import { temporaryDndState, temporaryDndStatus } from "./temporaryDnd";

const inactive = { active: false, until: null, remainingMs: 0, deadlineMonoMs: null };
const active = { active: true, until: "2026-10-23T09:30:00Z", remainingMs: 1_800_000, deadlineMonoMs: 1_900_000 };

test("临时状态缺失或损坏时不把未知展示成已关闭", () => {
  assert.equal(temporaryDndState({}), null);
  for (const patch of [{ remainingMs: NaN }, { remainingMs: -1 }, { remainingMs: 3_600_001 },
    { remainingMs: 0 }, { until: "invalid" }, { deadlineMonoMs: null }]) {
    assert.equal(temporaryDndState({ temporaryDoNotDisturb: { ...active, ...patch } }), null);
  }
  assert.match(temporaryDndStatus({}), /暂未提供/);
});

test("取消临时来源后仍明确显示其它免打扰来源", () => {
  assert.match(temporaryDndStatus({ temporaryDoNotDisturb: inactive, effectiveDoNotDisturb: true }), /仍受手动或每日/);
  assert.equal(temporaryDndStatus({ temporaryDoNotDisturb: inactive, effectiveDoNotDisturb: false }), "临时免打扰未开启");
});

test("前端显示原生剩余时间，不依据本机日期擅自结束临时来源", () => {
  assert.match(temporaryDndStatus({ temporaryDoNotDisturb: active }), /30 分钟/);
  assert.match(temporaryDndStatus({ temporaryDoNotDisturb: { ...active, remainingMs: 1 } }), /1 分钟/);
  assert.equal(temporaryDndState({ temporaryDoNotDisturb: active })?.active, true);
});
