import type {
  Query,
  Session,
  Snapshot,
  UsageDashboardData,
  UsageDashboardGroup,
  UsageDashboardRequest,
  UsageDashboardRow,
  UsageDashboardScopeGroup,
  UsagePriceVersion,
} from "./contracts";
import { demoSnapshot } from "./demo";
import { localDate } from "./format";

/** Hand-authored browser preview only. All totals are derived from these same rows. */
export function demoUsageDashboard(
  snapshot: Snapshot,
  query: Query,
  request: UsageDashboardRequest,
): UsageDashboardData {
  const canonical = demoSnapshot();
  const empty =
    snapshot.daily.length === 0 || snapshot.overview.responseCount === 0;
  if (empty) canonical.daily = [];
  const today = localDate();
  const examplePrice: UsagePriceVersion = {
    id: "demo-price-sol-1",
    revision: 1,
    model: "gpt-6.1-sol",
    providerId: null,
    effectiveFrom: "2020-01-01T00:00:00+08:00",
    createdAt: "2020-01-01T00:00:00+08:00",
    inputPerMillion: "2.5",
    outputPerMillion: "10",
    cacheReadPerMillion: "0.25",
    cacheWritePerMillion: "2.5",
  };
  const portion = (total: number, index: number, count: number) =>
    Math.floor(total / count) + (index < total % count ? 1 : 0);
  const allRows: UsageDashboardRow[] = [];
  // Keep authored identities separate from the public rows so privacy does not
  // collapse groups and these identities cannot escape through paginated rows.
  const rowSessions = new Map<string, Session>();
  for (const day of canonical.daily) {
    for (let index = 0; index < day.responseCount; index++) {
      const session = canonical.sessions[index % canonical.sessions.length];
      const turnIndex = canonical.sessions.indexOf(session);
      if (
        (query.sourceId && query.sourceId !== "demo-source") ||
        (query.clientKind && query.clientKind !== "codex") ||
        (query.providerId && query.providerId !== "unknown") ||
        (query.model && query.model !== session.model) ||
        (query.project && query.project !== session.project) ||
        (query.threadId && query.threadId !== session.id) ||
        (query.agentKind && query.agentKind !== session.agentKind) ||
        (query.rootTurnId &&
          query.rootTurnId !==
            (turnIndex === 1 ? "demo-turn-0" : `demo-turn-${turnIndex}`)) ||
        (query.from && day.date < query.from) ||
        (query.to && day.date > query.to)
      )
        continue;
      const input = portion(day.inputTokens, index, day.responseCount),
        output = portion(day.outputTokens, index, day.responseCount),
        cached = Math.min(
          input,
          portion(day.cachedInputTokens ?? 0, index, day.responseCount),
        );
      const normal = input - cached;
      // Integer nanodollars under this explicitly illustrative price version.
      const costNanos = normal * 2500 + output * 10000 + cached * 250;
      const isPriced = session.model === examplePrice.model;
      const nowHour = Number(
        new Intl.DateTimeFormat("en-GB", {
          timeZone: "Asia/Shanghai",
          hour: "2-digit",
          hour12: false,
        }).format(new Date()),
      );
      const hour =
        day.date === today
          ? Math.floor(
              (index * Math.min(17, nowHour)) /
                Math.max(1, day.responseCount - 1),
            )
          : index % 18;
      const id = `demo-response-${day.date}-${index}`;
      rowSessions.set(id, session);
      allRows.push({
        id,
        sourceId: snapshot.settings.privacyMode ? null : "demo-source",
        threadId: snapshot.settings.privacyMode ? null : session.id,
        responseId: snapshot.settings.privacyMode
          ? null
          : `demo-response-${day.date}-${index}`,
        eventTime: `${day.date}T${String(hour).padStart(2, "0")}:00:00+08:00`,
        clientKind: "codex",
        providerId: null,
        requestedModel: session.model,
        observedModel: session.model,
        inputTokens: input,
        normalInputTokens: normal,
        outputTokens: output,
        cachedInputTokens: cached,
        cacheWriteInputTokens: 0,
        totalTokens: input + output,
        reasoningOutputTokens: Math.floor(output * 0.6),
        estimatedCostUsd: isPriced ? (costNanos / 1e9).toFixed(9) : null,
        pricingVersionId: isPriced ? examplePrice.id : null,
        priceReason: isPriced ? null : "no_matching_price",
        turnDurationMs: null,
        turnTimeToFirstTokenMs: null,
      });
    }
  }
  allRows.sort(
    (a, b) =>
      b.eventTime.localeCompare(a.eventTime) || a.id.localeCompare(b.id),
  );
  const sum = (
    rows: UsageDashboardRow[],
    key:
      | "inputTokens"
      | "outputTokens"
      | "totalTokens"
      | "cachedInputTokens"
      | "normalInputTokens"
      | "reasoningOutputTokens",
  ) => rows.reduce((total, row) => total + (row[key] ?? 0), 0);
  const cost = (rows: UsageDashboardRow[]): string | null => {
    const priced = rows.filter((row) => row.estimatedCostUsd != null);
    return priced.length
      ? (
          priced.reduce(
            (total, row) =>
              total + Math.round(Number(row.estimatedCostUsd) * 1e9),
            0,
          ) / 1e9
        ).toFixed(9)
      : null;
  };
  const aggregate = (
    key: string,
    name: string,
    rows: UsageDashboardRow[],
  ): UsageDashboardGroup => ({
    key,
    name,
    responseCount: rows.length,
    totalTokens: sum(rows, "totalTokens"),
    inputTokens: sum(rows, "inputTokens"),
    outputTokens: sum(rows, "outputTokens"),
    cachedInputTokens: rows.length ? sum(rows, "cachedInputTokens") : null,
    cacheHitRate: sum(rows, "inputTokens")
      ? sum(rows, "cachedInputTokens") / sum(rows, "inputTokens")
      : null,
    cacheCoverage: sum(rows, "inputTokens") ? 1 : null,
    estimatedCostUsd: cost(rows),
    pricedResponses: rows.filter((row) => row.estimatedCostUsd != null).length,
    turnTimeToFirstTokenMs: null,
    timedResponses: 0,
    successRate: null,
    speedTokensPerSecond: null,
  });
  const groups = (
    key: "requestedModel" | "providerId",
  ): UsageDashboardGroup[] => {
    const map = new Map<string, UsageDashboardRow[]>();
    for (const row of allRows) {
      const name = row[key] ?? "unknown";
      const group = map.get(name) ?? [];
      group.push(row);
      map.set(name, group);
    }
    return [...map]
      .map(([name, rows]) =>
        aggregate(name, name === "unknown" ? "未知" : name, rows),
      )
      .sort(
        (a, b) =>
          b.responseCount - a.responseCount || a.key.localeCompare(b.key),
      );
  };
  // Opaque grouping keys are presentation keys, never navigation identities.
  const opaqueKey = (value: string): string => {
    let hash = 0xcbf29ce484222325n;
    for (const character of value) {
      hash ^= BigInt(character.codePointAt(0)!);
      hash = BigInt.asUintN(64, hash * 0x100000001b3n);
    }
    return hash.toString(16).padStart(16, "0");
  };
  const sessionIdentity = (session: Session) =>
    `${session.sourceId ?? "demo-source"}\u0000${session.id}`;
  const scopeGroups = (
    scope: "project" | "session",
  ): UsageDashboardScopeGroup[] => {
    const map = new Map<string, UsageDashboardRow[]>();
    for (const row of allRows) {
      const session = rowSessions.get(row.id)!;
      const identity =
        scope === "project" ? session.project : sessionIdentity(session);
      const rows = map.get(identity) ?? [];
      rows.push(row);
      map.set(identity, rows);
    }
    return [...map]
      .map(([identity, rows]) => {
        const session = rowSessions.get(rows[0].id)!;
        const project = canonical.projects.find(
          (item) => item.id === session.project,
        );
        const key = `demo-${scope}-${opaqueKey(identity)}`;
        const times = rows.map((row) => row.eventTime).sort();
        const sources = new Set(
          rows.map((row) => rowSessions.get(row.id)!.sourceId ?? "demo-source"),
        );
        return {
          ...aggregate(
            key,
            scope === "project"
              ? (project?.name ?? session.projectName ?? "未知项目")
              : session.title,
            rows,
          ),
          sourceId:
            !snapshot.settings.privacyMode && sources.size === 1
              ? [...sources][0]
              : null,
          threadId:
            !snapshot.settings.privacyMode && scope === "session"
              ? session.id
              : null,
          projectId: snapshot.settings.privacyMode ? null : session.project,
          sessionCount: new Set(
            rows.map((row) => sessionIdentity(rowSessions.get(row.id)!)),
          ).size,
          firstEventTime: times[0] ?? null,
          lastEventTime: times.at(-1) ?? null,
        };
      })
      .sort(
        (a, b) => b.totalTokens - a.totalTokens || a.key.localeCompare(b.key),
      )
      .map((group, index) => ({
        ...group,
        name: snapshot.settings.privacyMode
          ? `${scope === "project" ? "项目" : "会话"} ${index + 1}`
          : group.name,
      }));
  };
  const dailyRows = new Map<string, UsageDashboardRow[]>();
  for (const row of allRows) {
    const date = row.eventTime.slice(0, 10);
    const rows = dailyRows.get(date) ?? [];
    rows.push(row);
    dailyRows.set(date, rows);
  }
  const forecastBasis = {
    daily: [...dailyRows]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([date, rows]) => ({
        date,
        totalTokens: sum(rows, "totalTokens"),
        responseCount: rows.length,
      })),
    // This preview has session lifecycle examples, but no explicit response ->
    // turn association. Do not infer completion from a shared thread or assign
    // whole-day response totals to those lifecycle examples.
    completedTurns: [],
    omittedIncompleteTurns: 0,
  };
  const hourly = !!query.from && query.from === query.to;
  const bucketRows = new Map<string, UsageDashboardRow[]>();
  if (hourly)
    for (let hour = 0; hour < 24; hour++)
      bucketRows.set(
        `${query.from}T${String(hour).padStart(2, "0")}:00:00+08:00`,
        [],
      );
  else
    for (const day of canonical.daily)
      if (
        (!query.from || day.date >= query.from) &&
        (!query.to || day.date <= query.to)
      )
        bucketRows.set(day.date, []);
  for (const row of allRows) {
    const bucket = hourly
      ? `${row.eventTime.slice(0, 13)}:00:00+08:00`
      : row.eventTime.slice(0, 10);
    const rows = bucketRows.get(bucket) ?? [];
    rows.push(row);
    bucketRows.set(bucket, rows);
  }
  const trend = [...bucketRows]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([bucket, rows]) => ({
      bucket,
      label: hourly
        ? bucket.slice(11, 16)
        : bucket.slice(5, 10).replace("-", "/"),
      totalTokens: sum(rows, "totalTokens"),
      responseCount: rows.length,
      estimatedCostUsd: cost(rows),
      pricedResponses: rows.filter((row) => row.estimatedCostUsd != null)
        .length,
    }));
  const input = sum(allRows, "inputTokens"),
    priced = allRows.filter((row) => row.estimatedCostUsd != null).length;
  const filteredRows = allRows.filter(
    (row) =>
      request.quality === "all" ||
      (row.estimatedCostUsd == null
        ? request.quality === "unpriced"
        : request.quality === "priced"),
  );
  const pageSize = [20, 50, 100].includes(request.pageSize)
    ? request.pageSize
    : 20;
  const page = request.page;
  return {
    asOf: new Date().toISOString(),
    request: { ...request, page, pageSize },
    totalRows: filteredRows.length,
    summary: {
      inputTokens: allRows.length ? input : null,
      outputTokens: allRows.length ? sum(allRows, "outputTokens") : null,
      totalTokens: allRows.length ? sum(allRows, "totalTokens") : null,
      cachedInputTokens: allRows.length
        ? sum(allRows, "cachedInputTokens")
        : null,
      cacheWriteInputTokens: allRows.length ? 0 : null,
      reasoningOutputTokens: allRows.length
        ? sum(allRows, "reasoningOutputTokens")
        : null,
      normalInputTokens: allRows.length
        ? sum(allRows, "normalInputTokens")
        : null,
      responseCount: allRows.length,
      trustedMeteredResponseCount: allRows.length,
      observedResponseCount: allRows.length,
      excludedResponses: 0,
      cacheHitRate: input ? sum(allRows, "cachedInputTokens") / input : null,
      cacheCoverage: input ? 1 : null,
      cacheWriteCoverage: input ? 1 : null,
      normalInputCoverage: input ? 1 : null,
      legacyTokens: 0,
      legacySegments: 0,
      estimatedCostUsd: cost(allRows),
      pricedResponses: priced,
      unpricedResponses: allRows.length - priced,
    },
    granularity: hourly ? "hour" : "day",
    trend,
    rows: filteredRows.slice(page * pageSize, (page + 1) * pageSize),
    models: groups("requestedModel"),
    providers: groups("providerId"),
    projects: scopeGroups("project"),
    sessions: scopeGroups("session"),
    forecastBasis,
    pricingVersions: empty ? [] : [examplePrice],
  };
}
