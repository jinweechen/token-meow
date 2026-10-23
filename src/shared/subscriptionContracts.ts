/** Subscription facts and derived values are produced by Core, never inferred in the UI. */
export type SubscriptionFreshness = "fresh" | "stale" | "unavailable";
export type SubscriptionQuality = "learning" | "conditional" | "validated" | "invalidated";
export type SubscriptionModelDimension = "response" | "request";

export interface SubscriptionRisk {
  state: string;
  label: string;
  exhaustsAtLower: string | null;
  exhaustsAtUpper: string | null;
  notifyEligible: boolean;
  reasons: string[];
}

export interface NativeQuotaForecast extends SubscriptionRisk {
  kind: "native_quota_eta";
  scope: { sourceId: string; windowId: string; periodInstanceId: string | null; runtimeId: string };
  asOf: string | null;
  asOfRevision: number | null;
}

export interface SubscriptionWindow {
  id: string;
  bucketId: string;
  bucketName: string;
  label: string;
  periodInstanceId: string | null;
  usedPercent: number | null;
  remainingPercent: number | null;
  resetsAt: string | null;
  resetState: "current" | "waiting" | "unknown";
  freshness: SubscriptionFreshness;
  canBindPlan: boolean;
  reasons: string[];
  risk?: SubscriptionRisk | null;
}

export interface SubscriptionModelUsage {
  model: string | null;
  modelDimension: SubscriptionModelDimension;
  tokens: number | null;
  from: string | null;
  to: string | null;
  coverage: string;
  reasons: string[];
}

/** Empirical local usage scenarios, never official quota or confidence bounds. */
export interface LocalPlanForecast {
  kind: "local_plan_eta" | "period_end_usage";
  state: "unavailable" | "unknown" | "conditional" | "reached";
  scope: "local-plan";
  planId: string;
  sourceId: string;
  modelDimension: SubscriptionModelDimension;
  model: string | null;
  planConfigurationRevision: number;
  /** Revision of the persisted input ledger, independently of plan edits. */
  asOfRevision: number | null;
  revisionKind?: "subscription-domain-revision";
  inputLedgerFingerprint: string | null;
  asOf: string | null;
  periodEndsAt?: string;
  quality: "conditional-local-experiment";
  notifyEligible: false;
  uncertaintyBounds: null;
  empiricalScenarioRange: {
    earliestAt?: string;
    latestAt?: string;
    minimum?: number;
    maximum?: number;
    unit?: string;
  } | null;
  relationToPeriodEnd: "before" | "after" | "overlapping" | null;
  willReachBeforePeriodEnd: boolean | null;
  reasons: string[];
}

export interface UsagePlan {
  id: string;
  revision: number;
  title: string;
  type: "tokens" | "percent";
  sourceId: string;
  model: string | null;
  modelDimension: SubscriptionModelDimension;
  target: number;
  effectiveFrom: string;
  endPolicy: "time" | "window";
  endsAt: string | null;
  windowId: string | null;
  lifecycle: "active" | "ended";
  evaluationState: "ready" | "pending" | "unknown";
  remindersEnabled: boolean;
  used: number | null;
  remaining: number | null;
  progressPercent: number | null;
  reasons: string[];
  forecasts?: {
    localPlanEta: LocalPlanForecast;
    periodEndUsage: LocalPlanForecast;
  };
}

export interface SaveUsagePlanInput {
  id?: string;
  expectedRevision: number | null;
  title: string;
  type: "tokens" | "percent";
  sourceId: string;
  model: string | null;
  modelDimension: SubscriptionModelDimension;
  target: number;
  endPolicy: "time" | "window";
  endsAt: string | null;
  windowId: string | null;
  remindersEnabled: boolean;
}

export interface SetUsagePlanStatusInput {
  id: string;
  expectedRevision: number;
  remindersEnabled?: boolean;
  end?: boolean;
}

export interface QuotaCalibration {
  archived?: boolean;
  recordedAt?: string;
  id: string;
  windowId: string;
  model: string | null;
  label: string;
  quality: SubscriptionQuality;
  capacityLower: number | null;
  capacityUpper: number | null;
  /** Precision-unknown empirical scenario only; no bounded quota guidance. */
  empiricalCapacity?: number | null;
  remainingLower: number | null;
  remainingUpper: number | null;
  sampleCount: number;
  cycleCount: number;
  coverage: string;
  structure: string;
  reasons: string[];
}

export interface QuotaAssumption {
  id: string;
  revision: number;
  sourceId: string;
  windowId: string;
  model: string | null;
  capacityTokens: number;
  validUntil: string;
  note: string;
  manualScenario?: {
    active: boolean;
    provenance: "manual-assumption";
    quality: "conditional";
    remainingEstimate: number | null;
    remainingLower: number | null;
    remainingUpper: number | null;
    estimatePrecision: "bounded" | "unknown";
    constraintsVerified: false;
    notifyEligible: false;
    reasons: string[];
  };
}

export interface SaveQuotaAssumptionInput {
  id?: string;
  expectedRevision: number | null;
  sourceId: string;
  windowId: string;
  model: string | null;
  capacityTokens: number;
  validUntil: string;
  note: string;
}

export interface UsagePlanDraftInput {
  intent: "remaining" | "habit";
  sourceId: string;
  windowId: string | null;
  model: string | null;
  modelDimension: SubscriptionModelDimension;
  reservePercent: number;
}

export interface UsagePlanDraft {
  kind?: "historical_demand";
  plan: SaveUsagePlanInput | null;
  basis: string[];
  constraintsVerified: boolean;
  reasons: string[];
}

export interface SubscriptionEvidenceAvailability {
  schemaVersion: 1;
  sourceId: string | null;
  /** Local collection time of the earliest reconstructible receipt. */
  earliestAvailableAt: string | null;
  earliestRecordedFactAt: string | null;
  timeBasis: "collected-at-not-provider-fact-time";
  receiptCount: number;
  fullEnvelopeCount: number;
  referenceReceiptCount: number;
  unresolvedReferenceCount: number;
  payloadBytes: number;
  retentionDays: number;
  limits: { perSourceReceipts: number; perSourcePayloadBytes: number; totalPayloadBytes: number; maxDays: number };
  capacityTruncation: {
    scope: "all-subscription-evidence";
    lastAppliedAt: string | null;
    deletedByAge: number;
    deletedBySourceCount: number;
    deletedBySourceBytes: number;
    deletedByGlobalBytes: number;
    deletedTotal: number;
    baseDependenciesMissing: number;
  };
  integrityCheckedOnResolve: true;
  payloadIsSqliteDiskSize: false;
}

export interface SubscriptionOverview {
  schemaVersion: 1;
  /** Opaque Core context; writes from a cleared/changed context are rejected. */
  scopeToken?: string;
  learningStatus?: { status: "unavailable" | "insufficient" | "assembled"; reasonCodes: string[]; assembledPairs: number };
  evidenceAvailability?: SubscriptionEvidenceAvailability;
  revision: number;
  asOf: string;
  source: { id: string; label: string } | null;
  membership: {
    label: string;
    planType: string | null;
    subTier: string | null;
    source: string;
    freshness: SubscriptionFreshness;
    reasons: string[];
  } | null;
  capabilities: {
    officialQuota: boolean;
    manualPlans: boolean;
    capacityAssumptions: boolean;
    drafts: boolean;
    nativeNotifications: boolean;
  };
  windows: SubscriptionWindow[];
  forecastResults?: (NativeQuotaForecast | LocalPlanForecast)[];
  modelUsage: SubscriptionModelUsage[];
  plans: UsagePlan[];
  calibrations: QuotaCalibration[];
  assumptions: QuotaAssumption[];
  notifications: {
    channel: "panel" | "native";
    qualified: boolean;
    reasons: string[];
  };
  reasons: string[];
}
