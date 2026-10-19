import { invoke } from "@tauri-apps/api/core";
import type {
  AccountSnapshot,
  PetCursor,
  PetDetails,
  PetDetailsRequest,
  PetPulse,
  PetPanelRequest,
  PanelNavigationRequest,
  PanelNavigationReply,
  PetChatOpenRequest,
  PetChatOpenResult,
  Query,
  Snapshot,
  UsageDashboardRequest,
  UsagePriceInput,
  UsagePriceVersion,
  UsagePriceSyncConfig,
  UsagePriceSyncConfigInput,
} from "./contracts";
import { parseExportResult } from "./exportResult";
import type {
  DesktopCompanionState,
  CompanionLauncher,
} from "./desktopCompanion";

export const isDesktop =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
export const getSnapshot = (query: Query, includeTools = true) =>
  invoke<Snapshot>("get_snapshot", { query, includeTools });
export const getUsageDashboardSnapshot = (
  query: Query,
  request: UsageDashboardRequest,
  includeTools = true,
) => invoke<Snapshot>("get_usage_dashboard_snapshot", { query, request, includeTools });
export const saveUsagePrice = (payload: UsagePriceInput) =>
  invoke<UsagePriceVersion>("save_usage_price", { payload });
export const saveUsagePriceSync = (payload: UsagePriceSyncConfigInput) =>
  invoke<UsagePriceSyncConfig>("save_usage_price_sync", { payload });
export const applyUsagePriceSync = (payload: {
  revision: number;
  items: (Pick<
    UsagePriceInput,
    | "inputPerMillion"
    | "outputPerMillion"
    | "cacheReadPerMillion"
    | "cacheWritePerMillion"
  > & { catalogProvider: string; catalogModel: string })[];
  skipped: number;
}) => invoke<UsagePriceSyncConfig>("apply_usage_price_sync", { payload });
export const recordUsagePriceSyncError = (payload: {
  revision: number;
  error: string;
}) =>
  invoke<UsagePriceSyncConfig>("record_usage_price_sync_error", { payload });
export const getPetPulse = (cursor?: PetCursor | null) =>
  invoke<PetPulse>("get_pet_pulse", { cursor: cursor ?? null });
export const getPetDetails = (request: PetDetailsRequest) =>
  invoke<PetDetails>("get_pet_details", { request });
export const runAction = (
  name: string,
  payload: Record<string, unknown> = {},
) => invoke<unknown>("run_action", { name, payload });
export const openPanel = (request?: PetPanelRequest) =>
  invoke("open_panel", { request: request ?? null });
export const panelNavigation = (request: PanelNavigationRequest) =>
  invoke<PanelNavigationReply>("panel_navigation", { request });
export const openPetChat = (request: PetChatOpenRequest) =>
  invoke<PetChatOpenResult>("open_pet_chat", { request });
export const openThread = (sourceId: string, threadId: string) =>
  invoke("open_thread", { sourceId, threadId });
export type PetDragRequest = {
  clientX: number;
  clientY: number;
  viewportWidth: number;
  viewportHeight: number;
};
export const dragPet = (request: PetDragRequest) =>
  invoke("start_pet_drag", { request });
export const setPetInteraction = (options: {
  active: boolean;
  revision: number;
}) =>
  invoke<{ active: boolean; revision: number }>("set_pet_interaction", options);
export const exportCsv = async (query: Query) =>
  parseExportResult(await invoke<unknown>("export_csv", { query }));
export const refreshAccount = () => invoke<AccountSnapshot>("refresh_account");
export const resetPetPosition = () => invoke("reset_pet_position");
export const quitApp = () => invoke("quit_app");
export const getDesktopCompanionState = () =>
  invoke<DesktopCompanionState>("get_desktop_companion_state");
export const listCompanionLaunchers = () =>
  invoke<CompanionLauncher[]>("list_companion_launchers");
export const chooseCompanionLauncher = () =>
  invoke<CompanionLauncher | null>("choose_companion_launcher");
export const removeCompanionLauncher = (launcherId: string) =>
  invoke("remove_companion_launcher", { options: { launcherId } });
export const launchCompanionApp = (launcherId: string) =>
  invoke<{ accepted: boolean }>("launch_companion_app", {
    options: { launcherId },
  });
