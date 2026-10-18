import type { Query, UsageForecastBasis } from "./contracts";

const DAY_MS = 86_400_000;
const SHANGHAI_OFFSET_MS = 8 * 3_600_000;
const MAX_TOKENS = Number.MAX_SAFE_INTEGER;

export type ForecastDayHorizon = 1 | 3 | 7;
export type ForecastTurnHorizon = 1 | 5 | 10;
export type ForecastReason =
  | "blocked"
  | "invalid_as_of"
  | "invalid_date_range"
  | "missing_basis"
  | "today_outside_range"
  | "today_missing"
  | "early_today"
  | "too_few_responses"
  | "too_few_days"
  | "select_session"
  | "select_source"
  | "too_few_turns"
  | "invalid_horizon"
  | "numeric_overflow";

interface SampleInfo {
  sampleCount: number;
  sampleKeys: string[];
  excludedSampleCount: number;
}
export interface UnavailableForecast extends SampleInfo {
  status: "unavailable";
  reason: ForecastReason;
}
export interface ReadyForecast extends SampleInfo {
  status: "ready";
  estimatedTokens: number;
  lowTokens: number;
  highTokens: number;
}
export interface TodayForecast extends ReadyForecast {
  observedTokens: number;
  responseCount: number;
  elapsedHours: number;
  remainingTokens: number;
}
export interface TurnForecast extends ReadyForecast {
  sampleFrom: string;
  sampleTo: string;
  currentTokens: number | null;
  cumulativeTokens: number | null;
  cumulativeUnavailable: "missing_current" | "numeric_overflow" | null;
}
interface ForecastData {
  asOf: string;
  summary: { totalTokens: number | null };
  forecastBasis?: UsageForecastBasis;
}
export interface UsageForecasts {
  todayDate: string | null;
  futureFrom: string | null;
  futureTo: string | null;
  historyFrom: string | null;
  historyTo: string | null;
  dayHorizon: ForecastDayHorizon;
  turnHorizon: ForecastTurnHorizon;
  omittedIncompleteTurns: number | null;
  today: TodayForecast | UnavailableForecast;
  future: ReadyForecast | UnavailableForecast;
  turns: TurnForecast | UnavailableForecast;
}

function isCount(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}
function dateKeyTime(value: unknown): number | null {
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(value))
    return null;
  const time = Date.parse(`${value}T00:00:00.000Z`);
  return Number.isFinite(time) &&
    new Date(time).toISOString().slice(0, 10) === value
    ? time
    : null;
}
/** Explicit offsets keep results independent of the browser's local timezone. */
function timestamp(value: unknown): number | null {
  if (typeof value !== "string") return null;
  const parts =
    /^(\d{4}-\d{2}-\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.exec(
      value,
    );
  if (
    !parts ||
    dateKeyTime(parts[1]) == null ||
    +parts[2] > 23 ||
    +parts[3] > 59 ||
    +parts[4] > 59
  )
    return null;
  const time = Date.parse(value);
  return Number.isFinite(time) ? time : null;
}
function localDay(time: number): string | null {
  const day = new Date(time + SHANGHAI_OFFSET_MS).toISOString().slice(0, 10);
  return dateKeyTime(day) == null ? null : day;
}
function shiftDay(day: string, amount: number): string | null {
  const start = dateKeyTime(day);
  return start == null
    ? null
    : localDay(start + amount * DAY_MS - SHANGHAI_OFFSET_MS);
}
function unavailable(
  reason: ForecastReason,
  samples: SampleInfo = {
    sampleCount: 0,
    sampleKeys: [],
    excludedSampleCount: 0,
  },
): UnavailableForecast {
  return { status: "unavailable", reason, ...samples };
}
function safeSum(values: number[]): number | null {
  let sum = 0;
  for (const value of values) {
    if (!isCount(value) || sum > MAX_TOKENS - value) return null;
    sum += value;
  }
  return sum;
}
function roundedCount(value: number): number | null {
  if (!Number.isFinite(value) || value < 0 || value > MAX_TOKENS) return null;
  const rounded = Math.round(value);
  return isCount(rounded) ? rounded : null;
}

interface Sample {
  key: string;
  time: number;
  tokens: number;
  responses: number;
}
/** Duplicate identifiers cannot inflate sample size; conflicting copies are omitted. */
function uniqueSamples(rows: Sample[]): { rows: Sample[]; excluded: number } {
  const unique = new Map<string, Sample>();
  const conflicts = new Set<string>();
  let excluded = 0;
  for (const row of rows) {
    if (conflicts.has(row.key)) {
      excluded++;
      continue;
    }
    const previous = unique.get(row.key);
    if (!previous) {
      unique.set(row.key, row);
      continue;
    }
    excluded++;
    if (
      previous.time !== row.time ||
      previous.tokens !== row.tokens ||
      previous.responses !== row.responses
    ) {
      unique.delete(row.key);
      conflicts.add(row.key);
      excluded++;
    }
  }
  return { rows: [...unique.values()], excluded };
}
function estimate(
  samples: Sample[],
  horizon: number,
  excluded: number,
): ReadyForecast | UnavailableForecast {
  const info = {
    sampleCount: samples.length,
    sampleKeys: samples.map((row) => row.key),
    excludedSampleCount: excluded,
  };
  const total = safeSum(samples.map((row) => row.tokens));
  if (total == null || samples.length === 0)
    return unavailable("numeric_overflow", info);
  const estimatedTokens = roundedCount((total / samples.length) * horizon);
  const lowTokens = roundedCount(
    Math.min(...samples.map((row) => row.tokens)) * horizon,
  );
  const highTokens = roundedCount(
    Math.max(...samples.map((row) => row.tokens)) * horizon,
  );
  if (estimatedTokens == null || lowTokens == null || highTokens == null)
    return unavailable("numeric_overflow", info);
  return { status: "ready", ...info, estimatedTokens, lowTokens, highTokens };
}

/** Pure local extrapolation. Samples and the fixed snapshot time are the only clock/data inputs. */
export function calculateUsageForecasts(
  data: ForecastData,
  query: Query,
  options: {
    dayHorizon?: ForecastDayHorizon;
    turnHorizon?: ForecastTurnHorizon;
    blockedReason?: string | null;
  } = {},
): UsageForecasts {
  const requestedDayHorizon =
    options.dayHorizon === undefined ? 7 : options.dayHorizon;
  const requestedTurnHorizon =
    options.turnHorizon === undefined ? 5 : options.turnHorizon;
  const validDayHorizon = [1, 3, 7].includes(requestedDayHorizon);
  const validTurnHorizon = [1, 5, 10].includes(requestedTurnHorizon);
  const dayHorizon = validDayHorizon ? requestedDayHorizon : 7;
  const turnHorizon = validTurnHorizon ? requestedTurnHorizon : 5;
  const asOf = timestamp(data.asOf);
  const todayDate = asOf == null ? null : localDay(asOf);
  const base = {
    todayDate,
    futureFrom: todayDate ? shiftDay(todayDate, 1) : null,
    futureTo: todayDate ? shiftDay(todayDate, dayHorizon) : null,
    historyFrom: todayDate ? shiftDay(todayDate, -7) : null,
    historyTo: todayDate ? shiftDay(todayDate, -1) : null,
    dayHorizon,
    turnHorizon,
    omittedIncompleteTurns: isCount(data.forecastBasis?.omittedIncompleteTurns)
      ? data.forecastBasis!.omittedIncompleteTurns
      : null,
  };
  const allUnavailable = (reason: ForecastReason): UsageForecasts => ({
    ...base,
    today: unavailable(reason),
    future: unavailable(reason),
    turns: unavailable(reason),
  });
  if (options.blockedReason) return allUnavailable("blocked");
  if (asOf == null || !todayDate || !base.historyFrom || !base.futureTo)
    return allUnavailable("invalid_as_of");
  const from = query.from || undefined,
    to = query.to || undefined;
  if (
    (from && dateKeyTime(from) == null) ||
    (to && dateKeyTime(to) == null) ||
    (from && to && from > to)
  )
    return allUnavailable("invalid_date_range");
  if (
    !data.forecastBasis ||
    !Array.isArray(data.forecastBasis.daily) ||
    !Array.isArray(data.forecastBasis.completedTurns)
  )
    return allUnavailable("missing_basis");
  const inRange = (day: string) => (!from || day >= from) && (!to || day <= to);
  let invalidDays = 0;
  const rawDays: Sample[] = [];
  for (const row of data.forecastBasis.daily) {
    const time = dateKeyTime(row?.date);
    if (
      time == null ||
      !isCount(row?.totalTokens) ||
      !isCount(row?.responseCount) ||
      row.responseCount === 0
    ) {
      invalidDays++;
      continue;
    }
    // A future bucket and a zero-filled empty day are never historical observations.
    if (row.date > todayDate || !inRange(row.date)) continue;
    rawDays.push({
      key: row.date,
      time,
      tokens: row.totalTokens,
      responses: row.responseCount,
    });
  }
  const days = uniqueSamples(rawDays);
  const excludedDays = invalidDays + days.excluded;
  const todayRow = days.rows.find((row) => row.key === todayDate);
  const elapsedHours =
    (asOf + SHANGHAI_OFFSET_MS - dateKeyTime(todayDate)!) / 3_600_000;
  let today: TodayForecast | UnavailableForecast;
  const todayInfo = {
    sampleCount: todayRow ? 1 : 0,
    sampleKeys: todayRow ? [todayDate] : [],
    excludedSampleCount: excludedDays,
  };
  if (!inRange(todayDate))
    today = unavailable("today_outside_range", todayInfo);
  else if (!todayRow) today = unavailable("today_missing", todayInfo);
  else if (elapsedHours < 3) today = unavailable("early_today", todayInfo);
  else if (todayRow.responses < 3)
    today = unavailable("too_few_responses", todayInfo);
  else {
    const projected = roundedCount(todayRow.tokens * (24 / elapsedHours));
    today =
      projected == null
        ? unavailable("numeric_overflow", todayInfo)
        : {
            status: "ready",
            ...todayInfo,
            estimatedTokens: projected,
            lowTokens: projected,
            highTokens: projected,
            observedTokens: todayRow.tokens,
            responseCount: todayRow.responses,
            elapsedHours,
            remainingTokens: projected - todayRow.tokens,
          };
  }
  const history = days.rows
    .filter((row) => row.key >= base.historyFrom! && row.key < todayDate)
    .sort((a, b) => a.key.localeCompare(b.key));
  const historyInfo = {
    sampleCount: history.length,
    sampleKeys: history.map((row) => row.key),
    excludedSampleCount: excludedDays,
  };
  const future = !validDayHorizon
    ? unavailable("invalid_horizon", historyInfo)
    : history.length < 3
      ? unavailable("too_few_days", historyInfo)
      : estimate(history, dayHorizon, excludedDays);

  let invalidTurns = 0;
  const rawTurns: Sample[] = [];
  for (const row of data.forecastBasis.completedTurns) {
    const time = timestamp(row?.completedAt);
    if (
      !row ||
      typeof row.key !== "string" ||
      !row.key.trim() ||
      time == null ||
      !isCount(row.totalTokens) ||
      !isCount(row.responseCount) ||
      row.responseCount === 0
    ) {
      invalidTurns++;
      continue;
    }
    const day = localDay(time);
    if (time > asOf || !day || !inRange(day)) continue;
    rawTurns.push({
      key: row.key,
      time,
      tokens: row.totalTokens,
      responses: row.responseCount,
    });
  }
  const uniqueTurns = uniqueSamples(rawTurns);
  const turnsSample = uniqueTurns.rows
    .sort((a, b) => b.time - a.time || a.key.localeCompare(b.key))
    .slice(0, 20);
  const excludedTurns = invalidTurns + uniqueTurns.excluded;
  const turnsInfo = {
    sampleCount: turnsSample.length,
    sampleKeys: turnsSample.map((row) => row.key),
    excludedSampleCount: excludedTurns,
  };
  let turns: TurnForecast | UnavailableForecast;
  if (!query.threadId?.trim()) turns = unavailable("select_session", turnsInfo);
  else if (!query.sourceId?.trim())
    turns = unavailable("select_source", turnsInfo);
  else if (!validTurnHorizon) turns = unavailable("invalid_horizon", turnsInfo);
  else if (turnsSample.length < 3)
    turns = unavailable("too_few_turns", turnsInfo);
  else {
    const prediction = estimate(turnsSample, turnHorizon, excludedTurns);
    if (prediction.status === "unavailable") turns = prediction;
    else {
      const currentTokens = isCount(data.summary.totalTokens)
        ? data.summary.totalTokens
        : null;
      const cumulativeTokens =
        currentTokens == null
          ? null
          : safeSum([currentTokens, prediction.estimatedTokens]);
      turns = {
        ...prediction,
        sampleFrom: new Date(
          turnsSample[turnsSample.length - 1].time,
        ).toISOString(),
        sampleTo: new Date(turnsSample[0].time).toISOString(),
        currentTokens,
        cumulativeTokens,
        cumulativeUnavailable:
          currentTokens == null
            ? "missing_current"
            : cumulativeTokens == null
              ? "numeric_overflow"
              : null,
      };
    }
  }
  return { ...base, today, future, turns };
}
