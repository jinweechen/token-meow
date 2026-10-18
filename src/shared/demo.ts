import type {
  PetCursor,
  PetDetails,
  PetDetailsRequest,
  PetPulse,
  PetReadHeader,
  Query,
  Session,
  Snapshot,
} from "./contracts";
import { localDate, queryDates } from "./format";

let petDemoRevision = 0n;
function demoPetHeader(): PetReadHeader {
  const day = localDate(new Date());
  return {
    protocolVersion: 1,
    runtimeEpoch: "browser-demo-pet",
    revision: String(++petDemoRevision),
    day,
    asOf: new Date().toISOString(),
    sourceScopeRevision: `browser-demo-pet:${day}`,
  };
}
/** Authored preview data, independent of the full panel Snapshot. */
export function demoPetPulse(cursor?: PetCursor | null): PetPulse {
  const header = demoPetHeader();
  return {
    ...header,
    settings: {
      muted: true,
      doNotDisturb: false,
      effectiveDoNotDisturb: false,
      reducedMotion: false,
      privacyMode: false,
      petVisible: true,
      petScale: 1,
      workCareEnabled: true,
      careTone: "mixed",
      careIntervalMinutes: 25,
    },
    activity: { activeTurns: 2, uncertainTurns: 0, state: "active" },
    health: {
      lastSuccessfulScan: header.asOf,
      lastSourceEvent: header.asOf,
      issues: 0,
      conflicts: 0,
    },
    unreadCount: 3,
    prompts: [],
    promptFeed: {
      cursor: {
        runtimeEpoch: header.runtimeEpoch,
        sourceScopeRevision: header.sourceScopeRevision,
        day: header.day,
        sequence: "0",
      },
      baseline:
        !cursor ||
        cursor.runtimeEpoch !== header.runtimeEpoch ||
        cursor.sourceScopeRevision !== header.sourceScopeRevision ||
        cursor.day !== header.day,
      dropped: {
        overflow: 0,
        expired: 0,
        invalidTime: 0,
        future: 0,
        ineligible: 0,
        otherDay: 0,
      },
    },
  };
}
export function demoPetDetails(request: PetDetailsRequest): PetDetails {
  const header = demoPetHeader();
  if (
    request.runtimeEpoch !== header.runtimeEpoch ||
    request.sourceScopeRevision !== header.sourceScopeRevision ||
    request.day !== header.day
  )
    throw new Error("演示日期或来源已变化，请重新打开小卡");
  return {
    ...header,
    overview: { totalTokens: 4200, cacheHitRate: 0.65 },
    latestTurn: { timeToFirstTokenMs: 500 },
    recentChat: null,
    account: {
      sourceId: "browser-demo-source",
      status: "connected",
      collectedAt: header.asOf,
      buckets: [
        {
          id: "preview-window",
          name: "演示额度",
          primary: {
            usedPercent: 40,
            windowDurationMins: 300,
            resetsAt: Math.floor(Date.now() / 1000) + 3600,
          },
        },
      ],
    },
  };
}

export function emptySnapshot(): Snapshot {
  return {
    overview: {
      inputTokens: null,
      outputTokens: null,
      totalTokens: null,
      cachedInputTokens: null,
      cacheWriteInputTokens: null,
      reasoningOutputTokens: null,
      responseCount: 0,
      cacheHitRate: null,
      cacheCoverage: null,
      normalInputTokens: null,
      legacyTokens: 0,
      legacySegments: 0,
    },
    daily: [],
    projects: [],
    models: [],
    sessions: [],
    turns: [],
    tools: [],
    inbox: [],
    budgets: [],
    settings: {
      muted: true,
      privacyMode: false,
      doNotDisturb: false,
      alwaysOnTop: true,
      clickThrough: false,
      petScale: 1,
      retentionDays: 90,
      startAtLogin: false,
      reducedMotion: false,
      petVisible: true,
      workCareEnabled: true,
      careTone: "mixed",
      careIntervalMinutes: 25,
    },
    health: {
      lastSuccessfulScan: null,
      lastSourceEvent: null,
      issues: [],
      conflicts: 0,
      files: 0,
      unsupportedFiles: 0,
      coverageFrom: null,
      coverageTo: null,
      unknownModels: 0,
      sourceRoot: null,
    },
    activity: { activeTurns: 0, uncertainTurns: 0, state: "idle" },
    capabilities: {},
    account: {
      status: "unavailable",
      collectedAt: null,
      buckets: [],
      error: "尚未连接账户只读接口",
    },
  };
}

// Browser-only, clearly labeled illustrative data. Never returned by the desktop bridge.
export function demoSnapshot(query: Query = {}): Snapshot {
  // These authored demo sessions all carry numeric usage; Native sessions may not.
  const snapshot = {
    ...emptySnapshot(),
    sessions: [] as (Session & { totalTokens: number })[],
  };
  const now = new Date().toISOString();
  const total = 1_284_670,
    input = 1_092_840,
    cached = 876_910;
  const values = [86_320, 142_410, 114_790, 206_540, 178_850, 291_090, 264_670];
  const dates = queryDates(7);
  const first = new Date(`${dates.from}T00:00:00+08:00`);
  snapshot.overview = {
    inputTokens: input,
    outputTokens: total - input,
    totalTokens: total,
    cachedInputTokens: cached,
    cacheWriteInputTokens: 0,
    reasoningOutputTokens: 113_420,
    responseCount: 126,
    cacheHitRate: cached / input,
    cacheCoverage: 1,
    normalInputTokens: input - cached,
    legacyTokens: 0,
    legacySegments: 0,
  };
  snapshot.daily = values.map((value, index) => {
    const day = new Date(first);
    day.setUTCDate(day.getUTCDate() + index);
    return {
      date: localDate(day),
      totalTokens: value,
      inputTokens: Math.round(value * 0.85),
      outputTokens: Math.round(value * 0.15),
      cachedInputTokens: Math.round(value * 0.68),
      responseCount: 18,
    };
  });
  snapshot.projects = [
    {
      id: "token-miao",
      name: "Token 喵",
      path: "D:\\projects\\token-miao",
      totalTokens: 684_230,
      responseCount: 62,
    },
    {
      id: "studio",
      name: "创作工作室",
      path: "D:\\projects\\studio",
      totalTokens: 412_880,
      responseCount: 41,
    },
    {
      id: "lab",
      name: "日常实验室",
      path: "D:\\projects\\lab",
      totalTokens: 187_560,
      responseCount: 23,
    },
  ];
  snapshot.models = [
    {
      name: "gpt-6.1-sol",
      totalTokens: 1_024_670,
      inputTokens: 871_000,
      outputTokens: 153_670,
      responseCount: 103,
    },
    {
      name: "gpt-6-astra",
      totalTokens: 260_000,
      inputTokens: 221_840,
      outputTokens: 38_160,
      responseCount: 23,
    },
  ];
  snapshot.sessions = [
    {
      id: "demo-thread-1",
      agentKind: "primary",
      title: "让桌面伙伴动起来",
      cwd: snapshot.projects[0].path,
      project: "Token 喵",
      parentThreadId: null,
      rootThreadId: null,
      totalTokens: 394_120,
      responseCount: 37,
      model: "gpt-6.1-sol",
    },
    {
      id: "demo-thread-2",
      agentKind: "subagent",
      title: "设计用量统计面板",
      cwd: snapshot.projects[0].path,
      project: "Token 喵",
      parentThreadId: "demo-thread-1",
      rootThreadId: "demo-thread-1",
      totalTokens: 290_110,
      responseCount: 25,
      model: "gpt-6.1-sol",
    },
    {
      id: "demo-thread-3",
      agentKind: "primary",
      title: "梳理章节结构与编辑体验",
      cwd: snapshot.projects[1].path,
      project: "创作工作室",
      parentThreadId: null,
      rootThreadId: null,
      totalTokens: 412_880,
      responseCount: 41,
      model: "gpt-6-astra",
    },
    {
      id: "demo-thread-4",
      agentKind: "unknown",
      title: "验证本地数据导出",
      cwd: snapshot.projects[2].path,
      project: "日常实验室",
      parentThreadId: null,
      rootThreadId: null,
      totalTokens: 187_560,
      responseCount: 23,
      model: "gpt-6.1-sol",
    },
  ];
  snapshot.turns = snapshot.sessions.map((session, index) => ({
    id: `demo-turn-${index}`,
    threadId: session.id,
    rootTurnId: index === 1 ? "demo-turn-0" : null,
    status: index === 3 ? "unknown" : "completed",
    startedAt: new Date(Date.now() - (index + 1) * 3_600_000).toISOString(),
    completedAt: new Date(
      Date.now() - (index + 1) * 3_600_000 + 183_000,
    ).toISOString(),
    durationMs: [183_000, 96_200, 264_100, 45_000][index],
    timeToFirstTokenMs: index === 3 ? null : [1240, 890, 1820][index],
    model: session.model,
  }));
  snapshot.tools = [
    {
      id: "demo-tool-1",
      threadId: "demo-thread-1",
      turnId: "demo-turn-0",
      name: "exec_command",
      kind: "shell",
      status: "completed",
      exitCode: 0,
      isError: false,
      durationMs: 3140,
    },
    {
      id: "demo-tool-2",
      threadId: "demo-thread-2",
      turnId: "demo-turn-1",
      name: "apply_patch",
      kind: "file",
      status: "completed",
      exitCode: null,
      isError: false,
      durationMs: 122,
    },
    {
      id: "demo-tool-3",
      threadId: "demo-thread-3",
      turnId: "demo-turn-2",
      name: "read_file",
      kind: "mcp",
      status: "failed",
      exitCode: null,
      isError: true,
      durationMs: 835,
    },
  ];
  snapshot.inbox = [
    {
      id: "demo-inbox-1",
      kind: "turn_completed",
      threadId: "demo-thread-1",
      turnId: "demo-turn-0",
      title: "让桌面伙伴动起来",
      status: "completed",
      createdAt: snapshot.turns[0].completedAt!,
      read: false,
    },
    {
      id: "demo-inbox-2",
      kind: "turn_completed",
      threadId: "demo-thread-3",
      turnId: "demo-turn-2",
      title: "梳理章节结构与编辑体验",
      status: "completed",
      createdAt: snapshot.turns[2].completedAt!,
      read: false,
    },
    {
      id: "demo-inbox-3",
      kind: "turn_completed",
      threadId: "demo-thread-4",
      turnId: "demo-turn-3",
      title: "验证本地数据导出",
      status: "unknown",
      createdAt: snapshot.turns[3].completedAt!,
      read: true,
    },
  ];
  snapshot.budgets = [
    {
      id: "demo-budget-1",
      scope: "daily",
      target: null,
      threshold: 2_000_000,
      enabled: true,
      usedTokens: 264_670,
    },
  ];
  snapshot.health = {
    lastSuccessfulScan: now,
    lastSourceEvent: now,
    issues: [],
    conflicts: 0,
    files: 24,
    unsupportedFiles: 0,
    coverageFrom: dates.from,
    coverageTo: dates.to,
    unknownModels: 0,
    sourceRoot: "演示来源 · 未读取本机文件",
  };
  snapshot.capabilities = {
    nativeUsage: "演示样本",
    turnTiming: "演示样本",
    legacyUsage: "未提供",
    generationSpeed: "暂不可用",
  };
  snapshot.account = {
    status: "connected",
    collectedAt: now,
    authMode: "演示账户",
    accountScope: "demo",
    buckets: [
      {
        id: "demo",
        name: "Codex · 示例额度桶",
        primary: {
          usedPercent: 28,
          windowDurationMins: 300,
          resetsAt: Math.floor(Date.now() / 1000) + 7200,
        },
        secondary: {
          usedPercent: 42,
          windowDurationMins: 10_080,
          resetsAt: Math.floor(Date.now() / 1000) + 200_000,
        },
      },
    ],
  };
  snapshot.primarySourceId = "demo-source";
  snapshot.account.sourceId = "demo-source";
  for (const project of snapshot.projects)
    project.id = `path:${project.path.replaceAll("\\", "/").toLowerCase()}`;
  for (const session of snapshot.sessions) {
    const project = snapshot.projects.find(
      (item) => item.name === session.project,
    );
    session.projectName = project?.name;
    session.project = project?.id ?? "unknown";
    session.sourceId = "demo-source";
  }
  for (const turn of snapshot.turns) turn.sourceId = "demo-source";
  for (const tool of snapshot.tools) tool.sourceId = "demo-source";
  for (const entry of snapshot.inbox) entry.sourceId = "demo-source";
  snapshot.sources = [
    {
      id: "demo-source",
      root: "演示来源 · 未读取本机文件",
      clientKind: "codex",
      environment: "Windows 本机（演示）",
      enabled: true,
    },
  ];
  snapshot.filters = {
    clientKinds: ["codex"],
    providerIds: ["unknown"],
    sourceIds: ["demo-source"],
  };
  if (
    query.from ||
    query.to ||
    query.model ||
    query.project ||
    query.threadId ||
    query.agentKind ||
    query.providerId ||
    query.sourceId
  ) {
    const allTotal = snapshot.overview.totalTokens!;
    snapshot.sessions = snapshot.sessions.filter(
      (session) =>
        (!query.model || session.model === query.model) &&
        (!query.project || session.project === query.project) &&
        (!query.agentKind || session.agentKind === query.agentKind) &&
        (!query.threadId || session.id === query.threadId),
    );
    const ratio =
      snapshot.sessions.reduce((sum, session) => sum + session.totalTokens, 0) /
      allTotal;
    snapshot.daily = snapshot.daily
      .filter(
        (day) =>
          (!query.from || day.date >= query.from) &&
          (!query.to || day.date <= query.to),
      )
      .map((day) => ({
        ...day,
        totalTokens: Math.round(day.totalTokens * ratio),
        inputTokens: Math.round(day.inputTokens * ratio),
        outputTokens: Math.round(day.outputTokens * ratio),
        cachedInputTokens: Math.round((day.cachedInputTokens ?? 0) * ratio),
        responseCount: Math.round(day.responseCount * ratio),
      }));
    for (const day of snapshot.daily)
      day.outputTokens = day.totalTokens - day.inputTokens;
    const inputTokens = snapshot.daily.reduce(
        (sum, day) => sum + day.inputTokens,
        0,
      ),
      outputTokens = snapshot.daily.reduce(
        (sum, day) => sum + day.outputTokens,
        0,
      ),
      cachedInputTokens = snapshot.daily.reduce(
        (sum, day) => sum + (day.cachedInputTokens ?? 0),
        0,
      ),
      totalTokens = inputTokens + outputTokens;
    snapshot.overview = {
      ...snapshot.overview,
      inputTokens: totalTokens ? inputTokens : null,
      outputTokens: totalTokens ? outputTokens : null,
      totalTokens: totalTokens || null,
      cachedInputTokens: totalTokens ? cachedInputTokens : null,
      normalInputTokens: totalTokens ? inputTokens - cachedInputTokens : null,
      reasoningOutputTokens: totalTokens
        ? Math.round(outputTokens * 0.6)
        : null,
      cacheHitRate: inputTokens ? cachedInputTokens / inputTokens : null,
      cacheCoverage: inputTokens ? 1 : null,
      responseCount: snapshot.daily.reduce(
        (sum, day) => sum + day.responseCount,
        0,
      ),
    };
    const dateRatio = totalTokens / Math.max(1, allTotal * ratio);
    snapshot.sessions = snapshot.sessions.map((session) => ({
      ...session,
      totalTokens: Math.round(session.totalTokens * dateRatio),
      responseCount: Math.round(session.responseCount * dateRatio),
    }));
    snapshot.projects = snapshot.projects
      .filter((project) =>
        snapshot.sessions.some((session) => session.project === project.id),
      )
      .map((project) => ({
        ...project,
        totalTokens: snapshot.sessions
          .filter((session) => session.project === project.id)
          .reduce((sum, session) => sum + session.totalTokens, 0),
        responseCount: snapshot.sessions
          .filter((session) => session.project === project.id)
          .reduce((sum, session) => sum + session.responseCount, 0),
      }));
    snapshot.models = snapshot.models
      .filter((model) =>
        snapshot.sessions.some((session) => session.model === model.name),
      )
      .map((model) => ({
        ...model,
        totalTokens: snapshot.sessions
          .filter((session) => session.model === model.name)
          .reduce((sum, session) => sum + session.totalTokens, 0),
      }));
    const ids = new Set(snapshot.sessions.map((session) => session.id));
    snapshot.turns = snapshot.turns.filter(
      (turn) =>
        ids.has(turn.threadId) &&
        (!query.from || localDate(new Date(turn.completedAt!)) >= query.from) &&
        (!query.to || localDate(new Date(turn.completedAt!)) <= query.to),
    );
    snapshot.tools = snapshot.tools.filter((tool) => ids.has(tool.threadId));
    snapshot.inbox = snapshot.inbox.filter(
      (event) =>
        ids.has(event.threadId) &&
        (!query.from || localDate(new Date(event.createdAt)) >= query.from) &&
        (!query.to || localDate(new Date(event.createdAt)) <= query.to),
    );
  }
  return snapshot;
}
