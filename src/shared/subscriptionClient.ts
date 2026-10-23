import { invoke } from "@tauri-apps/api/core";
import type {
  SaveQuotaAssumptionInput,
  SaveUsagePlanInput,
  SetUsagePlanStatusInput,
  SubscriptionOverview,
  UsagePlanDraft,
  UsagePlanDraftInput,
} from "./subscriptionContracts";

export type SubscriptionInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export function createSubscriptionClient(call: SubscriptionInvoke) {
  // The caller binds each action to the view that produced its form. A late
  // read must never replace a shared scope and authorize an older form.
  const scoped = (payload: object, expectedScope?: string) => expectedScope ? { ...payload, expectedScope } : payload;
  return {
    readOverview: () => call<SubscriptionOverview>("read_subscription_overview"),
    savePlan: (payload: SaveUsagePlanInput, expectedScope?: string) => call<SubscriptionOverview>("save_usage_plan", { payload: scoped(payload, expectedScope) }),
    setPlanStatus: (payload: SetUsagePlanStatusInput, expectedScope?: string) => call<SubscriptionOverview>("set_usage_plan_status", { payload: scoped(payload, expectedScope) }),
    saveAssumption: (payload: SaveQuotaAssumptionInput, expectedScope?: string) => call<SubscriptionOverview>("save_quota_assumption", { payload: scoped(payload, expectedScope) }),
    generateDraft: (payload: UsagePlanDraftInput, expectedScope?: string) => call<UsagePlanDraft>("generate_usage_plan_draft", { payload: scoped(payload, expectedScope) }),
    // This explicit user action shares the existing throttled official reader.
    refreshOfficial: () => call<unknown>("refresh_account"),
  };
}

export type SubscriptionClient = ReturnType<typeof createSubscriptionClient>;
export const subscriptionClient = createSubscriptionClient(invoke);
