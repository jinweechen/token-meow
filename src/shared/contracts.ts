export type AgentKind = "primary" | "subagent" | "unknown";

export interface Query {
  from?: string;
  to?: string;
  model?: string;
  project?: string;
  threadId?: string;
  sourceId?: string;
  clientKind?: string;
  providerId?: string;
  rootTurnId?: string;
  agentKind?: AgentKind;
  includeLegacy?: boolean;
}
export interface Overview {
  inputTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens: number | null;
  reasoningOutputTokens: number | null;
  responseCount: number;
  cacheHitRate: number | null;
  cacheCoverage: number | null;
  normalInputTokens: number | null;
  legacyTokens: number;
  legacySegments: number;
  cacheWriteCoverage?: number | null;
  normalInputCoverage?: number | null;
  excludedResponses?: number;
  observedResponseCount?: number;
  trustedMeteredResponseCount?: number;
}
export interface UsageDashboardRequest {
  page: number;
  pageSize: number;
  quality: "all" | "priced" | "unpriced";
}
export interface UsagePriceInput {
  model: string;
  providerId: string | null;
  effectiveFrom: string;
  inputPerMillion: string;
  outputPerMillion: string;
  cacheReadPerMillion: string | null;
  cacheWritePerMillion: string | null;
}
export interface UsagePriceSyncMapping {
  catalogProvider: string;
  catalogModel: string;
  model: string;
  providerId: string | null;
}
export interface UsagePriceSyncConfigInput {
  revision: number;
  enabled: boolean;
  overwriteManual: boolean;
  mappings: UsagePriceSyncMapping[];
}
export interface UsagePriceSyncConfig extends UsagePriceSyncConfigInput {
  schemaVersion: 1;
  lastAttemptAt: string | null;
  lastSuccessAt: string | null;
  error: string | null;
  changed: number;
  skipped: number;
}
export interface UsagePriceVersion extends UsagePriceInput {
  id: string;
  revision: number;
  createdAt: string;
  origin?: {
    kind: "models.dev";
    catalogProvider: string;
    catalogModel: string;
    syncedAt: string;
  } | null;
}
export interface UsageDashboardRow {
  id: string;
  sourceId: string | null;
  threadId: string | null;
  responseId: string | null;
  eventTime: string;
  clientKind: string;
  providerId: string | null;
  requestedModel: string | null;
  observedModel: string | null;
  inputTokens: number | null;
  normalInputTokens: number | null;
  outputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens: number | null;
  totalTokens: number | null;
  reasoningOutputTokens: number | null;
  estimatedCostUsd: string | null;
  pricingVersionId: string | null;
  priceReason: string | null;
  turnDurationMs: number | null;
  turnTimeToFirstTokenMs: number | null;
}
export interface UsageDashboardGroup {
  key: string;
  name: string;
  responseCount: number;
  totalTokens: number;
  inputTokens: number;
  outputTokens: number;
  cachedInputTokens: number | null;
  cacheHitRate?: number | null;
  cacheCoverage?: number | null;
  estimatedCostUsd: string | null;
  pricedResponses: number;
  turnTimeToFirstTokenMs: number | null;
  timedResponses: number;
  successRate?: number | null;
  speedTokensPerSecond?: number | null;
}
export interface UsageDashboardScopeGroup extends UsageDashboardGroup {
  /** Navigation identities are unavailable when privacy mode is enabled. */
  sourceId: string | null;
  threadId: string | null;
  projectId: string | null;
  sessionCount: number;
  firstEventTime: string | null;
  lastEventTime: string | null;
}
export interface UsageForecastBasis {
  daily: { date: string; totalTokens: number; responseCount: number }[];
  completedTurns: {
    key: string;
    completedAt: string;
    totalTokens: number;
    responseCount: number;
  }[];
  omittedIncompleteTurns: number;
}
export interface UsageDashboardData {
  asOf: string;
  request: UsageDashboardRequest;
  totalRows: number;
  summary: Overview & {
    estimatedCostUsd: string | null;
    pricedResponses: number;
    unpricedResponses: number;
  };
  granularity: "hour" | "day";
  trend: {
    bucket: string;
    label: string;
    totalTokens: number;
    responseCount: number;
    estimatedCostUsd: string | null;
    pricedResponses: number;
  }[];
  rows: UsageDashboardRow[];
  models: UsageDashboardGroup[];
  providers: UsageDashboardGroup[];
  projects: UsageDashboardScopeGroup[];
  sessions: UsageDashboardScopeGroup[];
  forecastBasis: UsageForecastBasis;
  pricingVersions: UsagePriceVersion[];
}
export interface Daily {
  date: string;
  totalTokens: number;
  inputTokens: number;
  outputTokens: number;
  cachedInputTokens: number | null;
  responseCount: number;
}
export interface Project {
  id: string;
  name: string;
  path: string;
  totalTokens: number;
  responseCount: number;
}
export interface Model {
  name: string;
  totalTokens: number;
  inputTokens: number;
  outputTokens: number;
  responseCount: number;
}
export interface Session {
  id: string;
  title: string;
  cwd: string;
  project: string;
  projectName?: string;
  parentThreadId: string | null;
  agentKind?: AgentKind;
  rootThreadId: string | null;
  totalTokens: number | null;
  responseCount: number;
  model: string | null;
  sourceId?: string;
}
export interface Turn {
  sourceId?: string;
  id: string;
  threadId: string;
  rootTurnId: string | null;
  status: string;
  startedAt: string | null;
  completedAt: string | null;
  durationMs: number | null;
  timeToFirstTokenMs: number | null;
  model: string | null;
}
export interface Tool {
  sourceId?: string;
  id: string;
  threadId: string;
  turnId: string | null;
  name: string;
  kind: string;
  status: string;
  exitCode: number | null;
  isError: boolean | null;
  durationMs: number | null;
}
export interface InboxEvent {
  sourceId?: string;
  id: string;
  kind: string;
  threadId: string;
  turnId: string | null;
  title: string;
  status: string;
  createdAt: string;
  read: boolean;
  notifyEligible?: boolean;
  notificationState?: string;
  animationEligible?: boolean;
  animationAcknowledged?: boolean;
}
export interface Budget {
  sourceId?: string | null;
  id: string;
  scope: string;
  target?: string | null;
  threshold: number;
  enabled: boolean;
  usedTokens?: number | null;
}
export interface Settings {
  muted?: boolean;
  doNotDisturb?: boolean;
  privacyMode?: boolean;
  alwaysOnTop?: boolean;
  clickThrough?: boolean;
  alphaHitTest?: boolean;
  petScale?: number;
  retentionDays?: number;
  startAtLogin?: boolean;
  reducedMotion?: boolean;
  petVisible?: boolean;
  workCareEnabled?: boolean;
  idlePlayEnabled?: boolean;
  idleRoamingEnabled?: boolean;
  idleRoamRange?: "nearby" | "workArea";
  workAppLinkEnabled?: boolean;
  careTone?: "gentle" | "playful" | "mixed";
  careIntervalMinutes?: 25 | 45 | 60;
  dndStart?: string;
  dndEnd?: string;
  effectiveDoNotDisturb?: boolean;
  [key: string]: unknown;
}
export interface Health {
  history?: {
    status: "running" | "paused" | "cancelled" | "idle";
    from?: string | null;
    pendingFiles?: number | null;
  };
  lastSuccessfulScan: string | null;
  lastSourceEvent: string | null;
  issues: unknown[] | number;
  conflicts: unknown[] | number;
  files: number;
  unsupportedFiles: number | null;
  unsupportedKnownFiles?: number;
  coverageFrom: string | null;
  coverageTo: string | null;
  unknownModels: number;
  sourceRoot: string | null;
  [key: string]: unknown;
}
export interface RateWindow {
  usedPercent: number | null;
  windowDurationMins: number | null;
  resetsAt: number | null;
}
export interface AccountSnapshot {
  sourceId?: string;
  status: "unavailable" | "connected" | "stale" | "loading";
  collectedAt: string | null;
  lastAttemptAt?: string | null;
  error?: string | null;
  authMode?: string | null;
  accountScope?: string | null;
  buckets: {
    id: string;
    name: string;
    primary?: RateWindow | null;
    secondary?: RateWindow | null;
  }[];
  usage?: {
    summary?: {
      lifetimeTokens?: number | null;
      peakDailyTokens?: number | null;
      longestRunningTurnSec?: number | null;
      currentStreakDays?: number | null;
      longestStreakDays?: number | null;
    };
    dailyUsageBuckets?: { startDate: string; tokens: number | null }[] | null;
  } | null;
}
export interface SourceInstance {
  id: string;
  root: string;
  clientKind: string;
  enabled: boolean;
  environment?: string;
  createdAt?: string;
  primary?: boolean;
  runtime?: string;
  health?: {
    checkedAt?: string | null;
    pendingFiles?: number | null;
    issues?: number | null;
    files?: number | null;
    lastSuccessfulCheck?: string | null;
    lastEvent?: string | null;
    observedFrom?: string | null;
    observedTo?: string | null;
    unknownModels?: number | null;
    unsupportedFiles?: number | null;
  } | null;
}
export interface CollectionChannel {
  id: string;
  sourceId: string;
  kind: string;
  enabled?: boolean;
}
export interface FieldCapability {
  support?: string;
  runtime?: string | null;
  sourceRuntime?: string;
  version?: string | null;
  supportedVersion?: string | null;
  testedVersion?: string | null;
  channel?: string | null;
  granularity?: string | null;
  prerequisites?: string[];
  evidenceRefs?: string[];
  verifiedAt?: string | null;
  verificationScope?: string;
  versionPolicy?: string;
  evidence?: string;
  note?: string;
}
export interface SourceCapabilities {
  sourceId: string;
  channelId?: string;
  usage?: FieldCapability;
  cacheRead?: FieldCapability;
  cacheWrite?: FieldCapability;
  reasoning?: FieldCapability;
  turnCompletion?: FieldCapability;
  turnTiming?: FieldCapability;
  requestTiming?: FieldCapability;
  subagents?: FieldCapability;
  context?: FieldCapability;
  chatTarget?: FieldCapability;
  quota?: FieldCapability;
  accountUsage?: FieldCapability;
}
export interface Snapshot {
  usageDashboard?: UsageDashboardData;
  usagePriceSync?: UsagePriceSyncConfig;
  desktop?: {
    recoveryShortcut?: string;
    shortcutAvailable?: boolean;
    collectorError?: string | null;
    shellError?: string | null;
    watcherError?: string | null;
    renderPaused?: boolean;
    directory?: {
      status?: string;
      error?: string | null;
      lastSuccessAt?: string | null;
      count?: number;
      sourceId?: string;
    };
    notifications?: {
      status?: string;
      error?: string | null;
      lastAttemptAt?: string | null;
      eventCount?: number;
    };
  };
  overview: Overview;
  daily: Daily[];
  projects: Project[];
  models: Model[];
  sessions: Session[];
  turns: Turn[];
  tools: Tool[];
  /** Missing in legacy full snapshots; false means tool details were not requested. */
  toolsLoaded?: boolean;
  inbox: InboxEvent[];
  budgets: Budget[];
  settings: Settings;
  health: Health;
  activity: { activeTurns: number; uncertainTurns: number; state: string };
  capabilities: Record<string, unknown>;
  account?: AccountSnapshot;
  primarySourceId?: string;
  sources?: SourceInstance[];
  channels?: CollectionChannel[];
  sourceCapabilities?: SourceCapabilities[];
  filters?: {
    clientKinds: string[];
    providerIds: string[];
    sourceIds?: string[];
  };
}

/** Read models for the global pet; they never accept a panel Query. */
export interface PetPrompt {
  id: string;
  kind: string;
  status: string;
  createdAt: string;
  animationEligible: boolean;
}
export interface PetPresentation {
  settings: Settings;
  activity: Snapshot["activity"];
  health: Pick<
    Health,
    "lastSuccessfulScan" | "lastSourceEvent" | "issues" | "conflicts"
  >;
  unreadCount: number;
  prompts: readonly PetPrompt[];
  sourceScopeRevision: string;
}
export interface PetCursor {
  runtimeEpoch: string;
  sourceScopeRevision: string;
  day: string;
  /** Decimal sequence, kept as a string across IPC to avoid integer rounding. */
  sequence: string;
}
export interface PetReadHeader {
  protocolVersion: 1;
  runtimeEpoch: string;
  revision: string;
  day: string;
  asOf: string;
  sourceScopeRevision: string;
}
export interface PetPulse extends PetPresentation, PetReadHeader {
  promptFeed: {
    cursor: PetCursor;
    baseline: boolean;
    dropped: {
      overflow: number;
      expired: number;
      invalidTime: number;
      future: number;
      ineligible: number;
      otherDay: number;
    };
  };
}
export interface PetDetailsRequest {
  day: string;
  runtimeEpoch: string;
  sourceScopeRevision: string;
}
export interface PetChatContext {
  sourceId: string;
  threadId: string;
}
export interface PetPanelRequest {
  destination: "inbox";
  petDomain: PetDetailsRequest;
}
export type PanelNavigationRequest =
  | { action: "peek" }
  | { action: "ack"; revision: string }
  | { action: "cancel" };
export interface PanelNavigationReply {
  /** Native checked-u64 sequence; never convert to Number. */
  watermark: string;
  pending: (PetPanelRequest & { revision: string; remainingMs: number }) | null;
  retry?: true;
}
export interface PetChatOpenRequest {
  petDomain: PetDetailsRequest;
  chat: PetChatContext;
}
export type PetChatOpenResult =
  | { status: "submitted" }
  | { status: "copyRequired"; threadId: string; message: string };
export interface PetDetails extends PetReadHeader {
  overview: Pick<Overview, "totalTokens" | "cacheHitRate">;
  latestTurn: { timeToFirstTokenMs: number | null } | null;
  /** Current primary source, Shanghai today, verified metadata member. */
  recentChat: PetChatContext | null;
  /** Cached quota windows only; this request never starts an account RPC. */
  account?: AccountSnapshot;
}
export type PetState = "idle" | "active" | "complete" | "alert" | "sleep";
