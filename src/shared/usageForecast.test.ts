import assert from "node:assert/strict";
import test from "node:test";
import {
  calculateUsageForecasts,
  type ForecastDayHorizon,
  type ForecastTurnHorizon,
} from "./usageForecast";

function fixture(): Parameters<typeof calculateUsageForecasts>[0] {
  return {
    asOf: "2026-10-17T04:00:00Z",
    summary: { totalTokens: 10_000 },
    forecastBasis: {
      daily: [
        ...Array.from({ length: 7 }, (_, index) => ({
          date: `2026-10-${10 + index}`,
          totalTokens: (index + 1) * 100,
          responseCount: 3,
        })),
        { date: "2026-10-17", totalTokens: 900, responseCount: 6 },
      ],
      completedTurns: [100, 200, 300].map((tokens, index) => ({
        key: `turn-${index}`,
        completedAt: `2026-10-17T0${index}:00:00Z`,
        totalTokens: tokens,
        responseCount: 1,
      })),
      omittedIncompleteTurns: 2,
    },
  };
}

test("today uses Shanghai elapsed time from the fixed snapshot and includes observed cache totals", () => {
  const result = calculateUsageForecasts(fixture(), {
    threadId: "selected",
    sourceId: "source",
  });
  assert.equal(result.todayDate, "2026-10-17");
  assert.equal(result.today.status, "ready");
  if (result.today.status !== "ready") return;
  assert.equal(result.today.elapsedHours, 12);
  assert.equal(result.today.observedTokens, 900);
  assert.equal(result.today.estimatedTokens, 1800);
  assert.equal(result.today.remainingTokens, 900);
  assert.equal(result.today.responseCount, 6);
  assert.equal(result.omittedIncompleteTurns, 2);
});

test("equivalent explicit timestamp offsets produce the same forecast without a local clock dependency", () => {
  const utc = fixture();
  const local = fixture();
  local.asOf = "2026-10-17T12:00:00+08:00";
  assert.deepEqual(
    calculateUsageForecasts(utc, { threadId: "selected", sourceId: "source" }),
    calculateUsageForecasts(local, {
      threadId: "selected",
      sourceId: "source",
    }),
  );
});

test("future 1, 3, and 7 day scenarios use only the previous seven ended recorded days", () => {
  for (const days of [1, 3, 7] as const) {
    const input = fixture();
    input.forecastBasis!.daily.push({
      date: "2026-10-18",
      totalTokens: 999_999,
      responseCount: 50,
    });
    input.forecastBasis!.daily.push({
      date: "2026-10-09",
      totalTokens: 999_999,
      responseCount: 50,
    });
    const result = calculateUsageForecasts(input, {}, { dayHorizon: days });
    assert.equal(result.future.status, "ready");
    if (result.future.status !== "ready") continue;
    assert.equal(result.future.estimatedTokens, 400 * days);
    assert.equal(result.future.lowTokens, 100 * days);
    assert.equal(result.future.highTokens, 700 * days);
    assert.equal(result.future.sampleCount, 7);
    assert.deepEqual(
      result.future.sampleKeys,
      [10, 11, 12, 13, 14, 15, 16].map((day) => `2026-10-${day}`),
    );
    assert.equal(result.futureFrom, "2026-10-18");
    assert.equal(result.futureTo, `2026-10-${17 + days}`);
    assert.equal(result.historyFrom, "2026-10-10");
    assert.equal(result.historyTo, "2026-10-16");
  }
});

test("Shanghai midnight changes the day and never uses the current partial day as a historical sample", () => {
  const before = fixture();
  before.asOf = "2026-10-16T15:59:59Z";
  const a = calculateUsageForecasts(before, {});
  assert.equal(a.todayDate, "2026-10-16");
  assert.equal(a.future.sampleCount, 6);
  assert.equal(a.today.status, "ready");
  const after = fixture();
  after.asOf = "2026-10-16T16:00:00Z";
  const b = calculateUsageForecasts(after, {});
  assert.equal(b.todayDate, "2026-10-17");
  assert.equal(b.future.sampleCount, 7);
  assert.equal(b.today.status, "unavailable");
  if (b.today.status === "unavailable")
    assert.equal(b.today.reason, "early_today");
});

test("today requires three elapsed hours and at least three trusted responses", () => {
  const input = fixture();
  input.asOf = "2026-10-16T18:59:59Z";
  let result = calculateUsageForecasts(input, {});
  if (result.today.status === "unavailable")
    assert.equal(result.today.reason, "early_today");
  else assert.fail("early snapshot should not forecast");
  input.asOf = "2026-10-16T19:00:00Z";
  result = calculateUsageForecasts(input, {});
  assert.equal(result.today.status, "ready");
  input.forecastBasis!.daily.at(-1)!.responseCount = 2;
  result = calculateUsageForecasts(input, {});
  if (result.today.status === "unavailable")
    assert.equal(result.today.reason, "too_few_responses");
  else assert.fail("two responses should not forecast");
});

test("date filters retain their meaning for today, historical days, and completed turns", () => {
  const input = fixture();
  const previousDay = calculateUsageForecasts(input, {
    from: "2026-10-16",
    to: "2026-10-16",
    threadId: "selected",
    sourceId: "source",
  });
  assert.equal(previousDay.today.status, "unavailable");
  if (previousDay.today.status === "unavailable")
    assert.equal(previousDay.today.reason, "today_outside_range");
  assert.equal(previousDay.future.sampleCount, 1);
  assert.equal(previousDay.turns.sampleCount, 0);
  const today = calculateUsageForecasts(input, {
    from: "2026-10-17",
    to: "2026-10-17",
    threadId: "selected",
    sourceId: "source",
  });
  assert.equal(today.future.status, "unavailable");
  assert.equal(today.future.sampleCount, 0);
  assert.equal(today.today.status, "ready");
  assert.equal(today.turns.status, "ready");
  for (const query of [
    { from: "2026-10-18", to: "2026-10-17" },
    { from: "2026-02-30" },
    { to: "2026-10-17T00:00" },
  ]) {
    const invalid = calculateUsageForecasts(input, query);
    if (invalid.today.status === "unavailable")
      assert.equal(invalid.today.reason, "invalid_date_range");
    else assert.fail("invalid date range should not forecast");
  }
});

test("empty or missing days are omitted rather than imputed as zero, with at least three distinct observations", () => {
  const input = fixture();
  input.forecastBasis!.daily = [
    { date: "2026-10-14", totalTokens: 100, responseCount: 1 },
    { date: "2026-10-15", totalTokens: 300, responseCount: 1 },
    { date: "2026-10-16", totalTokens: 0, responseCount: 0 },
  ];
  let result = calculateUsageForecasts(input, {});
  assert.equal(result.future.status, "unavailable");
  assert.equal(result.future.sampleCount, 2);
  input.forecastBasis!.daily[2].responseCount = 1;
  result = calculateUsageForecasts(input, {});
  assert.equal(result.future.status, "ready");
  if (result.future.status === "ready") {
    assert.equal(result.future.estimatedTokens, 933);
    assert.equal(result.future.lowTokens, 0);
    assert.equal(result.future.highTokens, 2100);
  }
});

test("all observed zero samples produce zero predictions and preserve known zero cumulative usage", () => {
  const input = fixture();
  input.summary.totalTokens = 0;
  input.forecastBasis!.daily.forEach((row) => {
    row.totalTokens = 0;
  });
  input.forecastBasis!.completedTurns.forEach((row) => {
    row.totalTokens = 0;
  });
  const result = calculateUsageForecasts(input, {
    threadId: "selected",
    sourceId: "source",
  });
  assert.equal(result.today.status, "ready");
  assert.equal(result.future.status, "ready");
  assert.equal(result.turns.status, "ready");
  if (result.today.status === "ready")
    assert.equal(result.today.estimatedTokens, 0);
  if (result.future.status === "ready")
    assert.equal(result.future.lowTokens, 0);
  if (result.turns.status === "ready") {
    assert.equal(result.turns.estimatedTokens, 0);
    assert.equal(result.turns.currentTokens, 0);
    assert.equal(result.turns.cumulativeTokens, 0);
  }
});

test("missing, fractional, negative, nonfinite and unsafe token counts never become samples", () => {
  for (const bad of [
    null,
    undefined,
    NaN,
    Infinity,
    -1,
    1.5,
    Number.MAX_SAFE_INTEGER + 1,
  ]) {
    const input = fixture();
    input.forecastBasis!.daily.forEach((row) => {
      row.totalTokens = bad as number;
    });
    input.forecastBasis!.completedTurns.forEach((row) => {
      row.totalTokens = bad as number;
    });
    const result = calculateUsageForecasts(input, {
      threadId: "selected",
      sourceId: "source",
    });
    assert.equal(result.today.status, "unavailable", String(bad));
    assert.equal(result.future.status, "unavailable", String(bad));
    assert.equal(result.turns.status, "unavailable", String(bad));
    assert.equal(result.future.sampleCount, 0);
    assert.equal(result.turns.sampleCount, 0);
    assert.equal(result.future.excludedSampleCount, 8);
  }
});

test("turn predictions require a selected session and three complete recorded turns", () => {
  const noSession = calculateUsageForecasts(fixture(), {});
  if (noSession.turns.status === "unavailable")
    assert.equal(noSession.turns.reason, "select_session");
  else assert.fail("no selected session should not forecast turns");
  const noSource = calculateUsageForecasts(fixture(), {
    threadId: "same-short-id",
  });
  assert.equal(noSource.turns.status, "unavailable");
  if (noSource.turns.status === "unavailable")
    assert.equal(noSource.turns.reason, "select_source");
  else assert.fail("a session without its source must not combine sources");
  const input = fixture();
  input.forecastBasis!.completedTurns.pop();
  const two = calculateUsageForecasts(input, {
    threadId: "selected",
    sourceId: "source",
  });
  if (two.turns.status === "unavailable")
    assert.equal(two.turns.reason, "too_few_turns");
  else assert.fail("two turns should not forecast");
  assert.equal(two.omittedIncompleteTurns, 2);
});

test("extreme valid date boundaries decline unsupported future ranges without throwing", () => {
  for (const asOf of [
    "9999-12-31T23:59:59Z",
    "9999-12-31T00:00:00Z",
    "0000-01-01T00:00:00Z",
  ]) {
    const input = fixture();
    input.asOf = asOf;
    assert.doesNotThrow(() =>
      calculateUsageForecasts(input, {
        threadId: "selected",
        sourceId: "source",
      }),
    );
    const result = calculateUsageForecasts(input, {});
    assert.equal(result.future.status, "unavailable");
  }
});

test("turn 1, 5, and 10 horizon predictions display incremental and cumulative usage separately", () => {
  for (const turns of [1, 5, 10] as const) {
    const result = calculateUsageForecasts(
      fixture(),
      { threadId: "selected", sourceId: "source" },
      { turnHorizon: turns },
    );
    assert.equal(result.turns.status, "ready");
    if (result.turns.status !== "ready") continue;
    assert.equal(result.turns.estimatedTokens, 200 * turns);
    assert.equal(result.turns.lowTokens, 100 * turns);
    assert.equal(result.turns.highTokens, 300 * turns);
    assert.equal(result.turns.currentTokens, 10_000);
    assert.equal(result.turns.cumulativeTokens, 10_000 + 200 * turns);
    assert.equal(result.turns.cumulativeUnavailable, null);
    assert.equal(result.turns.sampleFrom, "2026-10-17T00:00:00.000Z");
    assert.equal(result.turns.sampleTo, "2026-10-17T02:00:00.000Z");
  }
});

test("turn samples are timestamp ordered, capped at the latest twenty, and exclude future completions", () => {
  const input = fixture();
  input.forecastBasis!.completedTurns = Array.from(
    { length: 25 },
    (_, index) => ({
      key: `turn-${index + 1}`,
      completedAt: `2026-10-16T12:${String(index).padStart(2, "0")}:00Z`,
      totalTokens: (index + 1) * 10,
      responseCount: 1,
    }),
  );
  input.forecastBasis!.completedTurns.reverse();
  input.forecastBasis!.completedTurns.push({
    key: "future",
    completedAt: "2026-10-18T00:00:00Z",
    totalTokens: 999_999,
    responseCount: 10,
  });
  const result = calculateUsageForecasts(input, {
    threadId: "selected",
    sourceId: "source",
  });
  assert.equal(result.turns.status, "ready");
  if (result.turns.status !== "ready") return;
  assert.equal(result.turns.sampleCount, 20);
  assert.equal(result.turns.sampleKeys[0], "turn-25");
  assert.equal(result.turns.sampleKeys.at(-1), "turn-6");
  assert.equal(result.turns.estimatedTokens, 775);
});

test("duplicate sample identities do not create observations and conflicting copies are omitted", () => {
  const input = fixture();
  const day = input.forecastBasis!.daily[0];
  input.forecastBasis!.daily = [day, { ...day }, { ...day }];
  const turn = input.forecastBasis!.completedTurns[0];
  input.forecastBasis!.completedTurns = [turn, { ...turn }, { ...turn }];
  let result = calculateUsageForecasts(input, {
    threadId: "selected",
    sourceId: "source",
  });
  assert.equal(result.future.sampleCount, 1);
  assert.equal(result.turns.sampleCount, 1);
  input.forecastBasis!.daily.push({ ...day, totalTokens: day.totalTokens + 1 });
  input.forecastBasis!.completedTurns.push({
    ...turn,
    totalTokens: turn.totalTokens + 1,
  });
  result = calculateUsageForecasts(input, {
    threadId: "selected",
    sourceId: "source",
  });
  assert.equal(result.future.sampleCount, 0);
  assert.equal(result.turns.sampleCount, 0);
});

test("safe arithmetic declines overflow instead of returning infinity or unsafe totals", () => {
  const input = fixture();
  input.forecastBasis!.daily.forEach((row) => {
    row.totalTokens = Number.MAX_SAFE_INTEGER;
  });
  input.forecastBasis!.completedTurns.forEach((row) => {
    row.totalTokens = Number.MAX_SAFE_INTEGER;
  });
  const result = calculateUsageForecasts(input, {
    threadId: "selected",
    sourceId: "source",
  });
  for (const value of [result.today, result.future, result.turns]) {
    assert.equal(value.status, "unavailable");
    if (value.status === "unavailable")
      assert.equal(value.reason, "numeric_overflow");
  }
  const cumulative = fixture();
  cumulative.summary.totalTokens = Number.MAX_SAFE_INTEGER;
  const output = calculateUsageForecasts(cumulative, {
    threadId: "selected",
    sourceId: "source",
  });
  assert.equal(output.turns.status, "ready");
  if (output.turns.status === "ready") {
    assert.equal(output.turns.cumulativeTokens, null);
    assert.equal(output.turns.cumulativeUnavailable, "numeric_overflow");
    assert.equal(output.turns.estimatedTokens, 1000);
  }
});

test("unknown current totals do not turn into zero cumulative forecasts", () => {
  const input = fixture();
  input.summary.totalTokens = null;
  input.forecastBasis!.omittedIncompleteTurns = undefined as unknown as number;
  const result = calculateUsageForecasts(input, {
    threadId: "selected",
    sourceId: "source",
  });
  assert.equal(result.omittedIncompleteTurns, null);
  assert.equal(result.turns.status, "ready");
  if (result.turns.status === "ready") {
    assert.equal(result.turns.currentTokens, null);
    assert.equal(result.turns.cumulativeTokens, null);
    assert.equal(result.turns.cumulativeUnavailable, "missing_current");
  }
});

test("invalid snapshot timestamps, missing basis, pending import, and illegal horizons explain nonprediction", () => {
  for (const asOf of [
    "invalid",
    "2026-02-30T12:00:00Z",
    "2026-10-17T12:00:00",
    "2026-10-17T24:00:00Z",
  ]) {
    const input = fixture();
    input.asOf = asOf;
    const result = calculateUsageForecasts(input, {});
    if (result.today.status === "unavailable")
      assert.equal(result.today.reason, "invalid_as_of");
    else assert.fail(asOf);
  }
  const noBasis = fixture();
  delete noBasis.forecastBasis;
  assert.deepEqual(calculateUsageForecasts(noBasis, {}).future, {
    status: "unavailable",
    reason: "missing_basis",
    sampleCount: 0,
    sampleKeys: [],
    excludedSampleCount: 0,
  });
  const blocked = calculateUsageForecasts(
    fixture(),
    {},
    { blockedReason: "历史正在导入" },
  );
  if (blocked.future.status === "unavailable")
    assert.equal(blocked.future.reason, "blocked");
  else assert.fail("pending imports must block");
  for (const bad of [0, 2, -1, NaN, Infinity, "3", null]) {
    const result = calculateUsageForecasts(
      fixture(),
      { threadId: "selected", sourceId: "source" },
      {
        dayHorizon: bad as ForecastDayHorizon,
        turnHorizon: bad as ForecastTurnHorizon,
      },
    );
    if (result.future.status === "unavailable")
      assert.equal(result.future.reason, "invalid_horizon");
    else assert.fail(String(bad));
    if (result.turns.status === "unavailable")
      assert.equal(result.turns.reason, "invalid_horizon");
    else assert.fail(String(bad));
  }
});

test("new query snapshots recalculate without retaining samples from a previous project or session", () => {
  const a = calculateUsageForecasts(fixture(), {
    project: "project-a",
    threadId: "session-a",
    sourceId: "source-a",
  });
  const other = fixture();
  other.summary.totalTokens = 0;
  other.forecastBasis!.daily.forEach((row) => {
    row.totalTokens = 0;
  });
  other.forecastBasis!.completedTurns.forEach((row) => {
    row.totalTokens = 0;
  });
  const b = calculateUsageForecasts(other, {
    project: "project-b",
    threadId: "session-b",
    sourceId: "source-b",
  });
  assert.equal(a.turns.status, "ready");
  assert.equal(b.turns.status, "ready");
  if (a.turns.status === "ready" && b.turns.status === "ready") {
    assert.equal(a.turns.estimatedTokens, 1000);
    assert.equal(b.turns.estimatedTokens, 0);
    assert.equal(b.turns.cumulativeTokens, 0);
  }
  const again = calculateUsageForecasts(fixture(), {
    project: "project-a",
    threadId: "session-a",
    sourceId: "source-a",
  });
  assert.deepEqual(again, a);
});
