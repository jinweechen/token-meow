import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { version as appVersion } from "../package.json";
import { invoke } from "@tauri-apps/api/core";
import type {
  Query,
  Settings,
  Snapshot,
  UsageDashboardRequest,
  UsagePriceInput,
  UsagePriceSyncConfigInput,
} from "./shared/contracts";
import * as bridge from "./shared/bridge";
import { demoSnapshot, emptySnapshot } from "./shared/demo";
import {
  compact,
  dateTime,
  duration,
  percent,
  accountExpired,
} from "./shared/format";
import { Badge, Icon, type IconName } from "./shared/UI";
import { UsageDashboard } from "./features/usage/UsageDashboard";
import { SubscriptionPanel } from "./features/subscription/SubscriptionPanel";
import { LocalSchedulePanel } from "./features/subscription/LocalSchedulePanel";
import { MixtureStudyPanel } from "./features/subscription/MixtureStudyPanel";
import { QueryFilters } from "./features/usage/QueryFilters";
import { AccountCard } from "./features/overview/Overview";
import { SourceAccountOverview } from "./features/account/SourceAccountOverview";
import { demoUsageDashboard } from "./shared/usageDashboardDemo";
import { usageCollectionCoverage, usageForecastBlockedReason } from "./shared/usageScope";
import { useUsagePriceSync } from "./features/usage/useUsagePriceSync";
import {
  HealthPanel,
  InboxPanel,
  SessionsPanel,
  SettingsPanel,
  ToolsPanel,
} from "./features/Panels";
import { CatGirl } from "./features/pet/CatGirl";
import { usePetGesture } from "./features/pet/usePetGesture";
import { PET_REACTIONS, type PetReaction } from "./features/pet/poseController";
import { WorkCareBubble } from "./features/pet/WorkCareBubble";
import { presentPet } from "./features/pet/presenter";
import { builtinAnimationManifest as builtinManifest } from "./features/pet/builtinAnimationSkin";
import { ViewRefreshLoop } from "./shared/viewRefreshLoop";
import { applyAgentKindFilter } from "./shared/filters";
import {
  QueryLifecycle,
  queryKey,
  queryScopeReady,
  type DatePeriod,
  type QueryUpdate,
} from "./shared/queryLifecycle";
import {
  snapshotViewKey,
  snapshotViewReady,
  usageDashboardRequestKey,
} from "./shared/snapshotView";
import {
  PanelNavigationController,
  PetCardActionController,
  capturePetCard,
  type PanelNavigationIntent,
} from "./shared/panelNavigation";
import { exportResultMessage } from "./shared/exportResult";
import {
  changesQueryDomain,
  enablesQueryPrivacy,
  isQueryDomainChange,
  revokeQueryFacts,
} from "./shared/queryDomain";
import { PetPopoverController, type PetPopover } from "./shared/petPopover";
import { usePetStore } from "./features/pet/usePetStore";
import { CompanionMenu } from "./features/companion/DesktopCompanion";
import { CompanionDropDialog } from "./features/companion/CompanionDropDialog";
import { useDesktopCompanion } from "./features/companion/useDesktopCompanion";
import { companionText } from "./shared/desktopCompanion";
import { useCurrentPetSkinRuntime } from "./features/pet/useLocalPetSkins";
import { localPetSkinClient } from "./features/pet/localPetSkinClient";
import { usePetExtensionCapabilities } from "./features/pet/usePetExtensionCapabilities";
import { TemporaryDoNotDisturbMenu } from "./features/notifications/TemporaryDoNotDisturb";
import { VrmPetCharacter } from "./features/pet/VrmPetCharacter";
import { useVrmSkins } from "./features/pet/useVrmSkins";
import { vrmSkinClient } from "./features/pet/vrmSkinClient";
import { ExtraPetApplication } from "./features/pet/ExtraPetApplication";
import { usePetPhysics } from "./features/pet/usePetPhysics";
import { PetPhysicsMenu } from "./features/pet/PetPhysicsMenu";

const navigation: {
  id: string;
  name: string;
  icon: IconName;
  caption: string;
}[] = [
  {
    id: "subscription",
    name: "订阅与计划",
    icon: "grid",
    caption: "查看会员额度、模型用量与使用计划。",
  },
  {
    id: "overview",
    name: "用量概览",
    icon: "grid",
    caption: "查看项目、会话与模型用量，估算未来的 Token 消耗。",
  },
  {
    id: "sessions",
    name: "会话管理",
    icon: "folder",
    caption: "管理会话、代理关系和项目归属，查看统计可进入用量概览。",
  },
  {
    id: "tools",
    name: "工具记录",
    icon: "terminal",
    caption: "保留事实，也保留结果的边界。",
  },
  {
    id: "inbox",
    name: "完成收件箱",
    icon: "inbox",
    caption: "忙完一个回合，留下一点回音。",
  },
  {
    id: "health",
    name: "来源与数据健康",
    icon: "heart",
    caption: "知道数据从哪里来，也知道哪里还没覆盖。",
  },
  {
    id: "settings",
    name: "偏好设置",
    icon: "settings",
    caption: "让小柚以你喜欢的方式陪伴。",
  },
];

function safeError(error: unknown): string {
  return typeof error === "string"
    ? error
    : error instanceof Error
      ? error.message
      : "本地操作未完成，请查看数据健康。";
}
function useApplication(dashboardEnabled: boolean, toolsEnabled: boolean) {
  const lifecycle = useRef<QueryLifecycle | null>(null);
  if (!lifecycle.current)
    lifecycle.current = new QueryLifecycle(window.location.search);
  const scope = lifecycle.current;
  const [queryState, setQueryState] = useState(() => scope.state());
  const query = queryState.query;
  const lastRequestedQuery = useRef(queryKey(query));
  const [acceptedScope, setAcceptedScope] = useState<{
    revision: number;
    query: Query;
  } | null>(() =>
    bridge.isDesktop ? null : { revision: queryState.revision, query },
  );
  const [dashboardRequest, setDashboardRequestState] =
    useState<UsageDashboardRequest>({ page: 0, pageSize: 20, quality: "all" });
  const dashboardView = useRef({
    enabled: dashboardEnabled,
    includeTools: toolsEnabled,
    request: dashboardRequest,
  });
  dashboardView.current = {
    enabled: dashboardEnabled,
    includeTools: toolsEnabled,
    request: dashboardRequest,
  };
  const viewKey = snapshotViewKey(dashboardView.current);
  const [acceptedViewKey, setAcceptedViewKey] = useState<string | null>(
    bridge.isDesktop ? null : viewKey,
  );
  const [snapshot, setSnapshot] = useState<Snapshot>(() =>
    bridge.isDesktop
      ? emptySnapshot()
      : {
          ...demoSnapshot(query),
          usageDashboard: dashboardEnabled
            ? demoUsageDashboard(demoSnapshot(query), query, dashboardRequest)
            : undefined,
        },
  );
  const queryReady =
    queryScopeReady(acceptedScope, queryState) &&
    snapshotViewReady(acceptedViewKey, dashboardView.current, snapshot);
  const [demo, setDemo] = useState(!bridge.isDesktop),
    [factsReady, setFactsReady] = useState(!bridge.isDesktop),
    [pending, setPending] = useState(false),
    [toast, setToast] = useState<string | null>(null),
    [connectionError, setConnectionError] = useState<string | null>(null),
    [subscriptionError, setSubscriptionError] = useState<string | null>(null);
  const demoRef = useRef(demo);
  const viewVisible = useRef(bridge.isDesktop ? false : !document.hidden),
    loop = useRef<ViewRefreshLoop | null>(null);
  demoRef.current = demo;
  const notify = useCallback((message: string) => setToast(message), []);
  const publishQuery = useCallback(() => {
    const state = scope.state();
    setQueryState((previous) =>
      previous.revision === state.revision && previous.period === state.period
        ? previous
        : state,
    );
  }, [scope]);
  const invalidateDomain = useCallback(
    (settings?: Settings) => {
      scope.domainChanged();
      publishQuery();
      setAcceptedScope(null);
      setAcceptedViewKey(null);
      setFactsReady(false);
      setSnapshot((previous) => revokeQueryFacts(previous, settings));
    },
    [scope, publishQuery],
  );
  const invalidateSettings = useCallback(
    (settings: unknown) => {
      if (enablesQueryPrivacy(settings)) invalidateDomain({ privacyMode: true });
      else scope.invalidate();
    },
    [scope, invalidateDomain],
  );
  const loadSnapshot = useCallback(async () => {
    scope.synchronize();
    publishQuery();
    if (bridge.isDesktop && (!viewVisible.current || !scope.canRead()))
      return;
    const ticket = scope.begin();
    if (lastRequestedQuery.current !== ticket.key) {
      lastRequestedQuery.current = ticket.key;
      if (dashboardView.current.request.page !== 0) {
        const next = { ...dashboardView.current.request, page: 0 };
        dashboardView.current = { ...dashboardView.current, request: next };
        setDashboardRequestState(next);
      }
    }
    const view = {
      ...dashboardView.current,
      request: { ...dashboardView.current.request },
    };
    const requestKey = snapshotViewKey(view);
    const viewIsCurrent = () =>
      requestKey === snapshotViewKey(dashboardView.current);
    publishQuery();
    if (!bridge.isDesktop) {
      if (scope.isCurrent(ticket) && viewIsCurrent()) {
        setSnapshot((previous) => {
          const next = {
            ...(demoRef.current ? demoSnapshot(ticket.query) : emptySnapshot()),
            settings: previous.settings,
            budgets: previous.budgets,
          };
          return {
            ...next,
            usageDashboard: view.enabled
              ? demoUsageDashboard(next, ticket.query, view.request)
              : undefined,
          };
        });
        setAcceptedScope({
          revision: ticket.revision,
          query: { ...ticket.query },
        });
        setAcceptedViewKey(requestKey);
      }
      publishQuery();
      return;
    }
    try {
      const data = view.enabled
        ? await bridge.getUsageDashboardSnapshot(ticket.query, view.request, view.includeTools)
        : await bridge.getSnapshot(ticket.query, view.includeTools);
      const revision = scope.state().revision;
      const current = scope.isCurrent(ticket);
      publishQuery();
      if (current && viewIsCurrent() && viewVisible.current) {
        if (!snapshotViewReady(requestKey, dashboardView.current, data))
          throw new Error("工具详情尚未完整载入，请重试。");
        setSnapshot(data);
        setAcceptedScope({
          revision: ticket.revision,
          query: { ...ticket.query },
        });
        setAcceptedViewKey(requestKey);
        setFactsReady(true);
        setConnectionError(null);
      } else if (viewVisible.current && revision !== scope.state().revision)
        void loop.current?.request(true);
    } catch (error) {
      const revision = scope.state().revision;
      const current = scope.isCurrent(ticket);
      publishQuery();
      if (current && viewIsCurrent() && viewVisible.current)
        setConnectionError(safeError(error));
      else if (viewVisible.current && revision !== scope.state().revision)
        void loop.current?.request(true);
    }
  }, [scope, publishQuery]);
  const refreshRef = useRef(loadSnapshot);
  refreshRef.current = loadSnapshot;
  // Mutations, retry buttons and filter changes share the same in-flight read
  // as polling. Their promises include one coalesced read after any old snapshot.
  const refresh = useCallback(
    () =>
      bridge.isDesktop
        ? (loop.current?.request(true) ?? Promise.resolve())
        : refreshRef.current(),
    [],
  );
  const priceSync = useUsagePriceSync(snapshot.usagePriceSync, refresh);
  const requestQueryRefresh = useCallback(() => {
    void refresh();
  }, [refresh]);
  const setDashboardRequest = useCallback(
    (next: UsageDashboardRequest) => {
      if (
        usageDashboardRequestKey(next) === usageDashboardRequestKey(dashboardView.current.request)
      )
        return;
      dashboardView.current = { ...dashboardView.current, request: next };
      setDashboardRequestState(next);
      scope.invalidate();
      requestQueryRefresh();
    },
    [scope, requestQueryRefresh],
  );
  const resetDashboardPage = useCallback(() => {
    if (dashboardView.current.request.page !== 0) {
      const next = { ...dashboardView.current.request, page: 0 };
      dashboardView.current = { ...dashboardView.current, request: next };
      setDashboardRequestState(next);
    }
  }, []);
  const setQuery = useCallback(
    (update: QueryUpdate) => {
      const revision = scope.state().revision;
      scope.update(update);
      publishQuery();
      if (scope.state().revision !== revision) {
        resetDashboardPage();
        requestQueryRefresh();
      }
    },
    [scope, publishQuery, requestQueryRefresh, resetDashboardPage],
  );
  const selectPeriod = useCallback(
    (period: DatePeriod) => {
      const revision = scope.state().revision;
      scope.selectPeriod(period);
      publishQuery();
      if (scope.state().revision !== revision) {
        resetDashboardPage();
        requestQueryRefresh();
      }
    },
    [scope, publishQuery, requestQueryRefresh, resetDashboardPage],
  );
  const openPetInbox = useCallback((day: string) => {
    if (!scope.openPetInbox(day)) return null;
    publishQuery();
    setAcceptedScope(null);
    setAcceptedViewKey(null);
    setFactsReady(false);
    resetDashboardPage();
    requestQueryRefresh();
    return scope.state();
  }, [scope, publishQuery, resetDashboardPage, requestQueryRefresh]);
  const reconcileQuery = useCallback(() => {
    const revision = scope.state().revision;
    scope.synchronize();
    publishQuery();
    if (scope.state().revision !== revision) requestQueryRefresh();
  }, [scope, publishQuery, requestQueryRefresh]);
  useEffect(() => {
    scope.invalidate();
    void refresh();
  }, [dashboardEnabled, toolsEnabled, scope, refresh]);
  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(() => setToast(null), 5500);
    return () => window.clearTimeout(timer);
  }, [toast]);
  useEffect(() => {
    if (!bridge.isDesktop) {
      const browserVisibility = () => {
        viewVisible.current = !document.hidden;
        scope.synchronize();
        publishQuery();
        if (!viewVisible.current) scope.invalidate();
        else void refresh();
      };
      document.addEventListener("visibilitychange", browserVisibility);
      return () => {
        scope.invalidate();
        document.removeEventListener("visibilitychange", browserVisibility);
      };
    }
    let live = true;
    let firstFrame = 0,
      secondFrame = 0;
    let nativeVisible = false,
      nativePaused = false;
    const label =
      new URLSearchParams(window.location.search).get("window") === "pet"
        ? "pet"
        : "main";
    const scheduler = new ViewRefreshLoop(
      () => refreshRef.current(),
      undefined,
      (error) => {
        if (live && viewVisible.current) setConnectionError(safeError(error));
      },
    );
    loop.current = scheduler;
    const applyVisibility = () => {
      const value = !document.hidden && nativeVisible && !nativePaused;
      scope.synchronize();
      publishQuery();
      if (!value) scope.invalidate();
      viewVisible.current = value;
      scheduler.setVisible(value);
    };
    const documentVisibility = () => applyVisibility();
    document.addEventListener("visibilitychange", documentVisibility);
    applyVisibility();
    const initialState = (event: Event) => {
      const detail = (
        event as CustomEvent<{ visible: boolean; paused: boolean }>
      ).detail;
      nativeVisible = detail.visible;
      nativePaused = detail.paused;
      window.dispatchEvent(
        new CustomEvent("view-visibility", {
          detail: { visible: nativeVisible },
        }),
      );
      window.dispatchEvent(
        new CustomEvent("desktop-runtime-state", {
          detail: { paused: nativePaused },
        }),
      );
      applyVisibility();
    };
    window.addEventListener("initial-view-state", initialState);
    const removers: (() => void)[] = [];
    const events = [
      "ledger-updated",
      "pet-options-changed",
      "account-updated",
      "source-accounts-updated",
      "pet-visibility-changed",
      "desktop-runtime-state",
      "window-visibility-changed",
    ];
    const subscriptions = events.map((event) =>
      listen(event, (payload) => {
        if (!live) return;
        if (event === "ledger-updated" && isQueryDomainChange(payload.payload))
          invalidateDomain();
        if (event === "pet-options-changed") invalidateSettings(payload.payload);
        if (event === "pet-visibility-changed" && label === "pet") {
          const detail = payload.payload as { visible?: boolean };
          nativeVisible = detail.visible !== false;
          applyVisibility();
          window.dispatchEvent(
            new CustomEvent("pet-visibility", { detail: payload.payload }),
          );
        }
        if (event === "window-visibility-changed") {
          const detail = payload.payload as {
            label?: string;
            visible?: boolean;
          };
          if (detail.label === label) {
            nativeVisible = detail.visible !== false;
            applyVisibility();
            window.dispatchEvent(
              new CustomEvent("view-visibility", {
                detail: { visible: nativeVisible },
              }),
            );
          }
        }
        if (event === "desktop-runtime-state") {
          nativePaused =
            (payload.payload as { paused?: boolean }).paused === true;
          applyVisibility();
          window.dispatchEvent(
            new CustomEvent("desktop-runtime-state", {
              detail: payload.payload,
            }),
          );
        }
        if (
          ![
            "window-visibility-changed",
            "pet-visibility-changed",
            "desktop-runtime-state",
          ].includes(event)
        )
          void scheduler.request(true);
      }).then((remove) => {
        if (!live) remove();
        else removers.push(remove);
      }),
    );
    // Subscribe before asking for initial visibility. The initial snapshot and
    // later events then enter the same lifecycle, including slow subscription setup.
    void Promise.allSettled(subscriptions).then((results) => {
      if (!live) return;
      const failed = results.find((result) => result.status === "rejected");
      setSubscriptionError(
        failed?.status === "rejected" ? safeError(failed.reason) : null,
      );
      firstFrame = requestAnimationFrame(() => {
        secondFrame = requestAnimationFrame(() => {
          void invoke<{ visible: boolean; paused: boolean }>("frontend_ready")
            .then((state) => {
              if (live && state)
                window.dispatchEvent(
                  new CustomEvent("initial-view-state", { detail: state }),
                );
            })
            .catch((error) => {
              if (live) setConnectionError(safeError(error));
            });
        });
      });
    });
    return () => {
      live = false;
      scope.invalidate();
      scheduler.dispose();
      if (loop.current === scheduler) loop.current = null;
      cancelAnimationFrame(firstFrame);
      cancelAnimationFrame(secondFrame);
      document.removeEventListener("visibilitychange", documentVisibility);
      window.removeEventListener("initial-view-state", initialState);
      removers.forEach((remove) => remove());
    };
  }, [scope, publishQuery, refresh, invalidateDomain, invalidateSettings]);
  async function action(name: string, payload: Record<string, unknown> = {}) {
    const domainMutation = bridge.isDesktop && changesQueryDomain(name);
    if (domainMutation) invalidateDomain();
    setPending(true);
    try {
      if (bridge.isDesktop) {
        try {
          if (domainMutation)
            await scope.withMutation(() => bridge.runAction(name, payload));
          else await bridge.runAction(name, payload);
        } finally {
          if (domainMutation) {
            // Fence reads started before the mutation committed, including a
            // notification received while another local mutation was pending.
            invalidateDomain();
          }
        }
        await refresh();
        notify("本地操作已完成");
      } else {
        setSnapshot((previous) => {
          if (name === "mark_read")
            return {
              ...previous,
              inbox: previous.inbox.map((item) =>
                payload.id == null || item.id === payload.id
                  ? { ...item, read: true }
                  : item,
              ),
            };
          if (name === "clear_inbox") return { ...previous, inbox: [] };
          if (name === "clear_data")
            return { ...emptySnapshot(), settings: previous.settings };
          if (name === "save_budget") {
            const budget = {
              id: String(payload.id ?? `preview-${Date.now()}`),
              scope: String(payload.scope ?? "daily"),
              sourceId:
                typeof payload.sourceId === "string" ? payload.sourceId : null,
              target: payload.target as string | null,
              threshold: Number(payload.threshold),
              enabled: payload.enabled !== false,
              usedTokens: 0,
            };
            return {
              ...previous,
              budgets: [
                ...previous.budgets.filter((item) => item.id !== budget.id),
                budget,
              ],
            };
          }
          if (name === "delete_budget")
            return {
              ...previous,
              budgets: previous.budgets.filter(
                (item) => item.id !== payload.id,
              ),
            };
          if (name === "set_project_mapping")
            return {
              ...previous,
              sessions: previous.sessions.map((item) =>
                item.cwd === payload.path
                  ? { ...item, project: String(payload.project) }
                  : item,
              ),
            };
          return previous;
        });
        notify("演示操作已应用；未读取或修改本机数据");
      }
    } catch (error) {
      if (domainMutation) void refresh();
      notify(safeError(error));
    } finally {
      setPending(false);
    }
  }
  async function save(settings: Settings) {
    if (bridge.isDesktop) invalidateSettings(settings);
    setPending(true);
    try {
      if (bridge.isDesktop) {
        await scope.withMutation(() =>
          bridge.runAction("update_settings", { settings }),
        );
        await refresh();
      } else
        setSnapshot((previous) => ({
          ...previous,
          settings: { ...previous.settings, ...settings },
        }));
      notify(
        bridge.isDesktop ? "偏好已保存" : "演示偏好已更新；不会保存到桌面应用",
      );
    } catch (error) {
      if (bridge.isDesktop) void refresh();
      notify(safeError(error));
    } finally {
      setPending(false);
    }
  }
  async function accountRefresh() {
    if (!bridge.isDesktop) {
      notify("浏览器预览没有账户读取权限；此处为演示快照");
      return;
    }
    setPending(true);
    try {
      await bridge.refreshAccount();
      // The backend projects account ownership for the current source filter.
      // A direct global refresh result could otherwise replace a secondary
      // source's unavailable account with the primary source's snapshot.
      await refresh();
    } catch (error) {
      notify(safeError(error));
    } finally {
      setPending(false);
    }
  }
  async function savePrice(payload: UsagePriceInput) {
    if (!bridge.isDesktop)
      throw new Error("浏览器演示不保存定价，请在桌面应用配置价格版本。");
    setPending(true);
    try {
      await bridge.saveUsagePrice(payload);
      scope.invalidate();
      await refresh();
      notify("价格版本已保存，估算费用已按响应发生时间重新计算");
    } finally {
      setPending(false);
    }
  }
  async function savePriceSync(payload: UsagePriceSyncConfigInput) {
    if (!bridge.isDesktop) throw new Error("浏览器演示不保存同步配置");
    await bridge.saveUsagePriceSync(payload);
    await refresh();
  }
  async function chat(id: string, sourceId?: string) {
    if (!bridge.isDesktop) {
      notify("演示会话无法跳转到真实聊天");
      return;
    }
    try {
      if (!id) {
        notify("此提醒没有会话上下文，请在主面板查看依据。");
        return;
      }
      if (!sourceId) throw Error("此记录缺少可验证的来源，不能自动跳转");
      await bridge.openThread(sourceId, id);
    } catch (error) {
      try {
        await navigator.clipboard.writeText(id);
        notify(`${safeError(error)} · 会话标识已复制`);
      } catch {
        notify(`${safeError(error)} · 会话标识：${id}`);
      }
    }
  }
  async function exportData() {
    // Export the effective current query, even before its new UI reply arrives.
    scope.synchronize();
    publishQuery();
    const effectiveQuery = scope.state().query;
    if (!bridge.isDesktop) {
      notify("浏览器预览不导出真实数据；请在桌面应用选择范围后导出");
      return;
    }
    try {
      const result = await bridge.exportCsv(effectiveQuery);
      const message = exportResultMessage(result);
      if (message) notify(message);
    } catch (error) {
      notify(safeError(error));
    }
  }
  function toggleDemo() {
    scope.synchronize();
    publishQuery();
    const value = !demoRef.current;
    setDemo(value);
    const state = scope.state();
    const next = value ? demoSnapshot(state.query) : emptySnapshot();
    setSnapshot({
      ...next,
      usageDashboard: dashboardView.current.enabled
        ? demoUsageDashboard(next, state.query, dashboardView.current.request)
        : undefined,
    });
    setAcceptedScope({ revision: state.revision, query: state.query });
    setAcceptedViewKey(snapshotViewKey(dashboardView.current));
  }
  async function resetPet() {
    if (!bridge.isDesktop) {
      notify("一键找回在 Windows 桌面应用中可用");
      return;
    }
    try {
      await bridge.resetPetPosition();
      await refresh();
      notify("小柚已回到可见工作区");
    } catch (error) {
      notify(safeError(error));
    }
  }
  return {
    snapshot,
    dashboardRequest,
    setDashboardRequest,
    savePrice,
    savePriceSync,
    priceSync,
    factsReady,
    queryReady,
    query,
    setQuery,
    period: queryState.period,
    selectPeriod,
    openPetInbox,
    queryRevision: queryState.revision,
    reconcileQuery,
    pending,
    toast,
    connectionError: connectionError ?? subscriptionError,
    demo,
    toggleDemo,
    refresh,
    notify,
    action,
    save,
    accountRefresh,
    chat,
    exportData,
    resetPet,
  };
}

export default function App() {
  const extraWindow = new URLSearchParams(window.location.search).get("window") === "companion";
  const petWindow =
    extraWindow || new URLSearchParams(window.location.search).get("window") === "pet";
  useEffect(() => {
    document.documentElement.classList.toggle("pet-mode", petWindow);
    document.body.classList.toggle("pet-mode", petWindow);
    return () => {
      document.documentElement.classList.remove("pet-mode");
      document.body.classList.remove("pet-mode");
    };
  }, [petWindow]);
  return extraWindow ? <ExtraPetApplication /> : petWindow ? <PetApplication /> : <MainApplication />;
}

function PetApplication() {
  const store = usePetStore();
  return <PetWindow store={store} />;
}

function MainApplication() {
  const [page, setPage] = useState("overview"),
    [sidebarOpen, setSidebarOpen] = useState(false);
  const store = useApplication(page === "overview", page === "tools");
  const panelNavigation = useRef<PanelNavigationController | null>(null);
  const latestApplication = useRef(store);
  latestApplication.current = store;
  const [inboxNavigation, setInboxNavigation] = useState<{
    intent: PanelNavigationIntent;
    queryRevision: number;
    queryKey: string;
  } | null>(null);
  const [mountedNavigation, setMountedNavigation] = useState<string | null>(null);
  const inboxMounted = useCallback((revision: string) => setMountedNavigation(revision), []);
  useEffect(() => {
    if (!bridge.isDesktop) return;
    const owner = new PanelNavigationController({
      now: () => performance.now(),
      listen: (wake) => listen("panel-navigation-request", wake),
      request: bridge.panelNavigation,
      apply: (intent) => {
        const next = latestApplication.current.openPetInbox(intent.petDomain.day);
        if (!next) return false;
        setMountedNavigation(null);
        setInboxNavigation({ intent, queryRevision: next.revision, queryKey: queryKey(next.query) });
        setPage("inbox");
        setSidebarOpen(false);
        return true;
      },
      error: (error) => latestApplication.current.notify(safeError(error)),
    });
    panelNavigation.current = owner;
    let nativeVisible: boolean | null = null, nativePaused: boolean | null = null;
    const applyVisibility = () => owner.setVisible(!document.hidden && nativeVisible === true && nativePaused === false);
    const initial = (event: Event) => {
      const detail = (event as CustomEvent<{ visible?: boolean; paused?: boolean }>).detail;
      nativeVisible = typeof detail?.visible === "boolean" ? detail.visible : null;
      nativePaused = typeof detail?.paused === "boolean" ? detail.paused : null;
      applyVisibility();
    };
    const visibility = (event: Event) => {
      const value = (event as CustomEvent<{ visible?: boolean }>).detail?.visible;
      nativeVisible = typeof value === "boolean" ? value : null;
      applyVisibility();
    };
    const runtime = (event: Event) => {
      const value = (event as CustomEvent<{ paused?: boolean }>).detail?.paused;
      nativePaused = typeof value === "boolean" ? value : null;
      applyVisibility();
    };
    const pagehide = () => owner.setVisible(false);
    document.addEventListener("visibilitychange", applyVisibility);
    window.addEventListener("initial-view-state", initial);
    window.addEventListener("view-visibility", visibility);
    window.addEventListener("desktop-runtime-state", runtime);
    window.addEventListener("pagehide", pagehide);
    return () => {
      owner.dispose();
      if (panelNavigation.current === owner) panelNavigation.current = null;
      document.removeEventListener("visibilitychange", applyVisibility);
      window.removeEventListener("initial-view-state", initial);
      window.removeEventListener("view-visibility", visibility);
      window.removeEventListener("desktop-runtime-state", runtime);
      window.removeEventListener("pagehide", pagehide);
    };
  }, []);
  useEffect(() => {
    if (inboxNavigation && mountedNavigation === inboxNavigation.intent.revision &&
        page === "inbox" && store.period === "1" && store.queryReady &&
        store.queryRevision === inboxNavigation.queryRevision &&
        queryKey(store.query) === inboxNavigation.queryKey)
      panelNavigation.current?.commit(inboxNavigation.intent.revision);
  }, [inboxNavigation, mountedNavigation, page, store.period, store.queryReady, store.queryRevision, store.query]);
  const cancelPanelNavigation = () => panelNavigation.current?.cancel();
  const manualPage = (next: string) => {
    cancelPanelNavigation();
    setInboxNavigation(null);
    setMountedNavigation(null);
    setPage(next);
    setSidebarOpen(false);
  };
  const manualQuery = (update: QueryUpdate) => {
    cancelPanelNavigation();
    store.setQuery(update);
  };
  const current = navigation.find((item) => item.id === page)!;
  const snapshot = store.snapshot;
  const privateMode = !!snapshot.settings.privacyMode,
    unread = store.queryReady
      ? snapshot.inbox.filter((item) => !item.read).length
      : 0;
  const exportPage = ["overview", "sessions", "tools"].includes(page);
  const exportScope = `${store.query.from ?? "起始日期"} 至 ${store.query.to ?? "当前日期"} · ${store.query.threadId ? "所选会话" : store.query.project ? "所选项目" : "当前筛选范围"}`;
  return (
    <div className={`app-shell ${sidebarOpen ? "sidebar-open" : ""}`}>
      <CompanionDropDialog />
      <aside className="sidebar">
        <a
          className="brand"
          href="#"
          onClick={(event) => {
            event.preventDefault();
            manualPage("overview");
          }}
        >
          <span className="brand-mark">
            <Icon name="cat" size={27} />
          </span>
          <span>
            <strong>
              Token 喵<span className="brand-dot">.</span>
            </strong>
            <small>YOUR QUIET COMPANION</small>
          </span>
        </a>
        <div className="sidebar-label">我的工作台</div>
        <nav aria-label="主导航">
          {navigation.map((item) => (
            <button
              className={`nav-item ${page === item.id ? "active" : ""}`}
              key={item.id}
              onClick={() => {
                manualPage(item.id);
              }}
            >
              <Icon name={item.icon} size={20} />
              <span>{item.name}</span>
              {item.id === "inbox" && unread > 0 && (
                <span className="nav-count">{unread}</span>
              )}
              {item.id === "overview" && <span className="active-indicator" />}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <div className="local-note">
            <Icon name="shield" size={18} />
            <strong>你的数据，留在本地。</strong>
            <p>
              只读采集，不上传对话，
              <br />
              也不额外调用模型。
            </p>
          </div>
          <div className="sidebar-status">
            <span
              className={`status-dot ${store.connectionError ? "amber" : ""}`}
            />
            <span>
              {bridge.isDesktop
                ? store.connectionError
                  ? "连接需检查"
                  : "本地桌面应用"
                : "浏览器演示预览"}
            </span>
            <span className="version">v{appVersion}</span>
          </div>
        </div>
      </aside>
      <div className="main-shell">
        <header className="topbar">
          <button
            className="icon-button mobile-menu"
            aria-label="展开导航"
            onClick={() => setSidebarOpen(!sidebarOpen)}
          >
            <Icon name="grid" />
          </button>
          <div className="breadcrumb">
            我的工作台
            <Icon name="chevron" size={13} />
            <strong>{current.name}</strong>
          </div>
          <div className="topbar-actions">
            {privateMode && (
              <Badge>
                <Icon name="eye" size={13} />
                隐私模式
              </Badge>
            )}
            <button
              className={`icon-button ${snapshot.settings.doNotDisturb ? "selected" : ""}`}
              title="手动免打扰"
              aria-label="切换手动免打扰"
              onClick={() =>
                void store.save({
                  doNotDisturb: !snapshot.settings.doNotDisturb,
                })
              }
            >
              <Icon name="moon" size={19} />
            </button>
            <span className="topbar-divider" />
            <span className="local-chip">
              <span className="status-dot" />
              {bridge.isDesktop ? "本机账本" : "DEMO"}
            </span>
            <span className="user-avatar">喵</span>
          </div>
        </header>
        <main className="main-content">
          {!bridge.isDesktop && (
            <div className="preview-banner">
              <Icon name="info" size={17} />
              <span>
                <strong>浏览器预览，未连接本机采集器。</strong>
                {store.demo
                  ? "当前数值为界面演示样本，与真实账户和用量无关。"
                  : "当前为空数据预览。"}
              </span>
              <button onClick={store.toggleDemo}>
                {store.demo ? "查看空数据" : "载入演示样本"}
              </button>
            </div>
          )}
          {store.connectionError && (
            <div className="error-banner">
              <Icon name="info" size={18} />
              <span>
                数据连接未完成：{store.connectionError}
                。保留上次快照；范围未就绪时不展示上一范围数字，不将缺失值填成
                0。
              </span>
              <button onClick={() => void store.refresh()}>重试</button>
            </div>
          )}
          <div className="page-heading">
            <div>
              <h1>{current.name}</h1>
              <p>{current.caption}</p>
            </div>
            {exportPage && (
              <div className="workspace-page-actions">
                <button
                  className="button secondary"
                  title={`导出响应用量：${exportScope}`}
                  onClick={() => void store.exportData()}
                >
                  <Icon name="download" size={17} />
                  导出 CSV
                </button>
                <span className="workspace-export-caption">{exportScope}</span>
              </div>
            )}
          </div>
          {["overview", "sessions", "tools", "inbox"].includes(page) && (
            <QueryFilters
              query={store.query}
              snapshot={snapshot}
              period={store.period}
              page={page}
              onQueryChange={manualQuery}
              onPeriodChange={(period) => { cancelPanelNavigation(); store.selectPeriod(period); }}
              onRefresh={() => void store.refresh()}
            />
          )}
          {!store.queryReady &&
            ["overview", "sessions", "tools", "inbox"].includes(page) && (
              <div className="notice" role="status" aria-live="polite">
                <Icon name="refresh" size={17} />
                <span>正在更新所选范围的数据…</span>
              </div>
            )}
          {page === "overview" && (
            <>
              <UsageDashboard
                data={
                  store.queryReady ? (snapshot.usageDashboard ?? null) : null
                }
                request={store.dashboardRequest}
                query={store.query}
                onSelectScope={manualQuery}
                collectionCoverage={usageCollectionCoverage(snapshot, store.query)}
                onOpenHealth={() => manualPage("health")}
                forecastBlockedReason={usageForecastBlockedReason(
                  snapshot,
                  store.query,
                )}
                onRequestChange={(request) => { cancelPanelNavigation(); store.setDashboardRequest(request); }}
                loading={!store.queryReady}
                error={store.connectionError}
                onSavePrice={store.savePrice}
                priceSyncConfig={snapshot.usagePriceSync}
                onSavePriceSyncConfig={store.savePriceSync}
                onSyncPricesNow={store.priceSync.syncNow}
                priceSyncing={store.priceSync.syncing}
                onOpenThread={(id, sourceId) => void store.chat(id, sourceId)}
                privacyMode={privateMode}
                isDemo={store.demo}
              />
              {store.queryReady && (
                <details className="dashboard-account-details">
                  <summary>账户额度 · 独立官方快照</summary>
                  <AccountCard
                    snapshot={snapshot}
                    onRefresh={() => void store.accountRefresh()}
                    pending={store.pending}
                  />
                  <SourceAccountOverview
                    sources={snapshot.sources ?? []}
                    privacyMode={privateMode}
                    scopeKey={`${store.queryRevision}:${privateMode}:${JSON.stringify((snapshot.sources ?? []).map(source => [source.id, source.root, source.clientKind, source.enabled, source.environment, source.configurationRevision]))}`}
                    enabled={bridge.isDesktop && store.queryReady && !store.demo}
                  />
                </details>
              )}
            </>
          )}
          {page === "subscription" && (
            <>
              <SubscriptionPanel
                sourceId={snapshot.primarySourceId}
                scopeKey={`${store.queryRevision}:${privateMode}`}
                enabled={bridge.isDesktop && store.queryReady && !store.pending}
                privacyMode={privateMode}
              />
              <LocalSchedulePanel
                sources={snapshot.sources ?? []}
                scopeKey={`${store.queryRevision}:${privateMode}`}
                enabled={bridge.isDesktop && store.queryReady && !store.pending && !store.demo}
                privacyMode={privateMode}
              />
              <MixtureStudyPanel
                sources={snapshot.sources ?? []}
                scopeKey={`${store.queryRevision}:${privateMode}`}
                enabled={bridge.isDesktop && store.queryReady && !store.pending && !store.demo}
                privacyMode={privateMode}
              />
            </>
          )}
          {page === "sessions" && store.queryReady && (
            <SessionsPanel
              snapshot={snapshot}
              agentKind={store.query.agentKind}
              onAgentKindChange={(value) =>
                manualQuery((previous) =>
                  applyAgentKindFilter(previous, value),
                )
              }
              onChat={(id, sourceId) => void store.chat(id, sourceId)}
              onUsage={(id, sourceId) => {
                manualQuery((previous) => ({
                  ...previous,
                  sourceId,
                  threadId: id,
                }));
                manualPage("overview");
              }}
              action={store.action}
            />
          )}
          {page === "tools" && store.queryReady && (
            <ToolsPanel
              snapshot={snapshot}
              onChat={(id, sourceId) => void store.chat(id, sourceId)}
            />
          )}
          {page === "inbox" && store.queryReady && (
            <InboxPanel
              key={inboxNavigation ? "pet-inbox:" + inboxNavigation.intent.revision : "ordinary-inbox"}
              initialFilter={inboxNavigation ? "unread" : "all"}
              navigationRevision={inboxNavigation?.intent.revision}
              onNavigationMounted={inboxMounted}
              onFilterChange={cancelPanelNavigation}
              onSubscription={() => setPage("subscription")}
              snapshot={snapshot}
              onChat={(id, sourceId) => void store.chat(id, sourceId)}
              action={store.action}
            />
          )}
          {page === "health" && (
            <HealthPanel
              snapshot={snapshot}
              action={store.action}
              onRefresh={() => void store.refresh()}
            />
          )}
          {page === "settings" && (
            <SettingsPanel
              snapshot={snapshot}
              save={store.save}
              action={store.action}
              onAccountRefresh={() => void store.accountRefresh()}
              pending={store.pending}
              onResetPet={() => void store.resetPet()}
            />
          )}
          <footer className="page-footer">
            <span>
              <Icon name="cat" size={15} />
              Token 喵 · 安静陪伴，清楚记录。
            </span>
            <span>
              Asia/Shanghai · 最后检查{" "}
              {dateTime(snapshot.health.lastSuccessfulScan)}
            </span>
          </footer>
        </main>
      </div>
      {store.toast && (
        <div className="toast" role="status">
          <Icon name="info" size={18} />
          {store.toast}
          <button
            aria-label="关闭提示"
            className="icon-button"
            onClick={() => store.notify("")}
          >
            <Icon name="close" size={16} />
          </button>
        </div>
      )}
    </div>
  );
}

function PetWindow({ store }: { store: ReturnType<typeof usePetStore> }) {
  const extensions = usePetExtensionCapabilities();
  const skins = useCurrentPetSkinRuntime(localPetSkinClient, extensions.enabled);
  const vrmSkins = useVrmSkins(vrmSkinClient, extensions.enabled);
  const companion = useDesktopCompanion();
  const [popover, setPopover] = useState<PetPopover>(null),
    [careTap, setCareTap] = useState(0),
    [stateText, setStateText] = useState("在这里，陪你慢慢创造");
  const controller = useRef<PetPopoverController | null>(null);
  const popoverElement = useRef<HTMLElement | null>(null);
  const shortcuts = useRef<PetCardActionController | null>(null);
  const [shortcutBusy, setShortcutBusy] = useState(false);
  const [shortcutsReady, setShortcutsReady] = useState(false);
  const latestCard = useRef({
    state: { ready: store.ready, pulse: store.presentation, detailsReady: store.detailsReady, details: store.details },
    notify: store.notify,
    refresh: store.refresh,
  });
  latestCard.current = {
    state: { ready: store.ready, pulse: store.presentation, detailsReady: store.detailsReady, details: store.details },
    notify: store.notify,
    refresh: store.refresh,
  };
  const presentation = store.presentation,
    intent = presentPet(presentation, store.ready),
    settings = presentation?.settings,
    details = store.ready && store.detailsReady ? store.details : null;
  const physics = usePetPhysics({
    enabled: extensions.enabled && (!vrmSkins.selected || !!vrmSkins.active) && (!skins.selected || !!skins.runtime),
    presentation, ready: store.ready,
    rendererKind: vrmSkins.active ? "vrm" : skins.runtime ? "png" : "builtin",
    resourceKey: vrmSkins.active ? `vrm:${vrmSkins.active.modelId}` : skins.runtime ? `png:${skins.runtime.skinId}` : "builtin",
  });
  const stageScale =
    typeof settings?.petScale === "number" && Number.isFinite(settings.petScale)
      ? Math.min(2, Math.max(0.5, settings.petScale))
      : 1;
  const supportsAffection = !!builtinManifest.variants?.tap.length;
  const accountWindow = details?.account?.buckets
    .flatMap((bucket) => [bucket.primary, bucket.secondary])
    .find(Boolean);
  const quotaExpired = accountExpired(details?.account);
  const delivered = useCallback((id: string) => {
    if (bridge.isDesktop)
      void bridge
        .runAction("acknowledge_delivery", { id })
        .catch(() => undefined);
  }, []);
  const interrupt = useCallback(
    () => window.dispatchEvent(new Event("pet-interaction")),
    [],
  );
  useEffect(() => {
    const owner = new PetCardActionController({
      desktop: bridge.isDesktop,
      now: () => Date.now(),
      current: () => latestCard.current.state,
      close: () => controller.current?.close(),
      interrupt,
      busy: setShortcutBusy,
      openInbox: bridge.openPanel,
      openChat: bridge.openPetChat,
      copy: (threadId) => navigator.clipboard.writeText(threadId),
      notify: (message) => latestCard.current.notify(message),
      refresh: () => latestCard.current.refresh(),
    });
    shortcuts.current = owner;
    setShortcutBusy(false);
    setShortcutsReady(false);
    let live = true, subscribed = !bridge.isDesktop;
    let nativeVisible: boolean | null = bridge.isDesktop ? null : true;
    let nativePaused: boolean | null = bridge.isDesktop ? null : false;
    let removeDomain: (() => void) | null = null;
    const applyVisibility = () => {
      const visible = subscribed && !document.hidden && nativeVisible === true && nativePaused === false;
      owner.setVisible(visible);
      if (live) setShortcutsReady(visible);
    };
    const initial = (event: Event) => {
      const detail = (event as CustomEvent<{ visible?: boolean; paused?: boolean }>).detail;
      nativeVisible = typeof detail?.visible === "boolean" ? detail.visible : null;
      nativePaused = typeof detail?.paused === "boolean" ? detail.paused : null;
      applyVisibility();
    };
    const visibility = (event: Event) => {
      const value = (event as CustomEvent<{ visible?: boolean }>).detail?.visible;
      nativeVisible = typeof value === "boolean" ? value : null;
      applyVisibility();
    };
    const runtime = (event: Event) => {
      const value = (event as CustomEvent<{ paused?: boolean }>).detail?.paused;
      nativePaused = typeof value === "boolean" ? value : null;
      applyVisibility();
    };
    const pagehide = () => { owner.setVisible(false); if (live) setShortcutsReady(false); };
    document.addEventListener("visibilitychange", applyVisibility);
    window.addEventListener("initial-view-state", initial);
    window.addEventListener("view-visibility", visibility);
    window.addEventListener("pet-visibility", visibility);
    window.addEventListener("desktop-runtime-state", runtime);
    window.addEventListener("pagehide", pagehide);
    if (bridge.isDesktop)
      void listen("ledger-updated", (event) => {
        if (live && isQueryDomainChange(event.payload)) owner.invalidate();
      }).then((remove) => {
        if (!live) remove();
        else { removeDomain = remove; subscribed = true; applyVisibility(); }
      }).catch((error) => { if (live) latestCard.current.notify(safeError(error)); });
    applyVisibility();
    return () => {
      live = false;
      owner.dispose();
      if (shortcuts.current === owner) shortcuts.current = null;
      removeDomain?.();
      document.removeEventListener("visibilitychange", applyVisibility);
      window.removeEventListener("initial-view-state", initial);
      window.removeEventListener("view-visibility", visibility);
      window.removeEventListener("pet-visibility", visibility);
      window.removeEventListener("desktop-runtime-state", runtime);
      window.removeEventListener("pagehide", pagehide);
    };
  }, [interrupt]);
  useEffect(() => {
    // Closing details alone is deliberately absent: this click owns its capture.
    shortcuts.current?.invalidate();
  }, [store.ready, presentation?.runtimeEpoch, presentation?.sourceScopeRevision, presentation?.day]);
  useEffect(() => {
    const owner = new PetPopoverController({
      window,
      document,
      change: (value) => {
        store.setDetailsOpen(value === "card");
        setPopover(value);
      },
      interrupt,
      error: (error) => store.notify(safeError(error)),
      interaction: bridge.isDesktop ? bridge.setPetInteraction : undefined,
      listenDismiss: bridge.isDesktop
        ? (callback) =>
            listen<{ reason?: string; revision?: number }>(
              "pet-popover-dismiss",
              (event) => callback(event.payload),
            )
        : undefined,
    });
    controller.current = owner;
    return () => {
      controller.current = null;
      store.setDetailsOpen(false);
      owner.dispose();
    };
  }, [interrupt, store.notify, store.setDetailsOpen]);
  useEffect(() => {
    if (!popover) return;
    popoverElement.current
      ?.querySelector<HTMLButtonElement>("button")
      ?.focus({ preventScroll: true });
  }, [popover]);
  const close = () => controller.current?.close();
  const affection = (pose: PetReaction = "affection") => {
    close();
    setCareTap((value) => value + 1);
    window.dispatchEvent(new CustomEvent("pet-affection", { detail: { pose } }));
  };
  const toggleCard = () => {
    controller.current?.toggle("card");
  };
  const open = async () => {
    close();
    interrupt();
    try {
      if (bridge.isDesktop) await bridge.openPanel();
      else window.open(window.location.pathname, "_blank", "noopener");
    } catch (error) { store.notify(safeError(error)); }
  };
  const canViewInbox = bridge.isDesktop && shortcutsReady && !shortcutBusy &&
    capturePetCard(latestCard.current.state, Date.now()) !== null;
  const canReturnChat = bridge.isDesktop && shortcutsReady && !shortcutBusy &&
    capturePetCard(latestCard.current.state, Date.now(), true) !== null;
  const petGesture = usePetGesture({ tap: affection, interrupt,
    drag: (origin) => {
      if (bridge.isDesktop) void bridge.dragPet(origin).catch((error) => store.notify(safeError(error)));
      else close();
    } });
  return (
    <div
      className="pet-window"
      style={
        {
          "--pet-stage-width": `${280 * stageScale}px`,
          "--pet-stage-height": `${340 * stageScale}px`,
        } as CSSProperties
      }
      data-pet-ready={store.ready}
      onContextMenu={(event) => {
        event.preventDefault();
        if (!(event.target as HTMLElement).closest(".pet-popover"))
          controller.current?.toggle("menu");
      }}
      onPointerDown={(event) => {
        if (
          event.button === 0 &&
          !(event.target as HTMLElement).closest(
            ".pet-popover, .pet-character-button",
          )
        )
          close();
      }}
    >
      {!bridge.isDesktop && <span className="pet-preview-tag">演示预览</span>}
      {supportsAffection && (
        <button
          className="pet-affection-button"
          aria-label="摸摸小柚"
          title="摸摸小柚 · 进行中的任务优先显示真实状态"
          onClick={() => affection()}
        >
          <Icon name="heart" size={15} />
        </button>
      )}
      <button
        {...petGesture()}
        className="pet-character-button"
        aria-label="小柚，单击摸摸，拖动移动，右键打开菜单"
        onKeyDown={(event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            affection();
          }
        }}
      >
        {vrmSkins.active ? <VrmPetCharacter key={`${vrmSkins.active.modelId}:${vrmSkins.revision}`}
          {...physics.rendererProps}
          model={vrmSkins.active} readModel={vrmSkinClient.readModel} presentation={presentation} ready={store.ready}
          onStateText={setStateText} onPromptDelivered={delivered} onFailure={vrmSkins.onFailure} /> : <CatGirl
          {...physics.rendererProps}
          presentation={presentation}
          skinRuntime={skins.runtime}
          pointerEnabled={extensions.enabled && !vrmSkins.selected}
          roamingEnabled={extensions.roaming && !skins.selected && !vrmSkins.selected}
          ready={store.ready}
          onStateText={setStateText}
          onPromptDelivered={delivered}
          companionMotionExpired={bridge.isDesktop && !companion.error && !popover && companion.motionExpired}
          companion={
            bridge.isDesktop
              ? companion.error || popover
                ? null
                : companion.state
              : undefined
          }
        />}
      </button>
      <div className="pet-status-badge" aria-live="polite">
        <span className={`status-dot ${intent.uncertain ? "amber" : ""}`} />
        <span>{stateText}</span>
        {store.ready && intent.badge && <b>{presentation?.unreadCount}</b>}
      </div>
      {vrmSkins.active && vrmSkins.active.meta.creditNotation !== "unnecessary" && <span className="pet-vrm-credit"
        title={`${vrmSkins.active.name} · ${vrmSkins.active.authors.join("、")}\n${typeof vrmSkins.active.meta.copyrightInformation === "string" ? vrmSkins.active.meta.copyrightInformation : ""}`}>
        {vrmSkins.active.authors.join("、")}
      </span>}
      {(skins.runtimeError || vrmSkins.runtimeError) && <span className="pet-health-badge" title={skins.runtimeError ?? vrmSkins.runtimeError ?? ""}>皮肤加载失败 · 已恢复内置角色</span>}
      {(intent.health || store.connectionError) && (
        <span className="pet-health-badge">
          {store.connectionError ? "活动数据待更新" : intent.health}
        </span>
      )}
      {companion.state &&
        !companion.error &&
        !popover &&
        !intent.health &&
        !store.connectionError &&
        settings?.privacyMode !== true && (
          <span className="pet-companion-context">
            {companionText(companion.state)}
          </span>
        )}
      <WorkCareBubble
        presentation={presentation}
        ready={store.ready}
        obscured={!!popover}
        manualRequest={careTap}
        onOpenPanel={() => void open()}
      />
      {popover === "card" && (
        <section
          ref={popoverElement}
          className="pet-popover pet-stats"
          role="dialog"
          aria-label="小柚的小账本"
        >
          <div className="section-heading">
            <h2>小柚的小账本</h2>
            <button
              className="icon-button"
              aria-label="关闭小卡"
              onClick={close}
            >
              <Icon name="close" size={15} />
            </button>
          </div>
          <dl className="metric-list">
            <div>
              <dt>今日总 token</dt>
              <dd>
                {details ? compact(details.overview.totalTokens) : "更新中"}
              </dd>
            </div>
            <div>
              <dt>加权缓存命中率</dt>
              <dd>
                {details ? percent(details.overview.cacheHitRate) : "更新中"}
              </dd>
            </div>
            <div>
              <dt>回合首 token 耗时</dt>
              <dd>
                {details
                  ? duration(details.latestTurn?.timeToFirstTokenMs)
                  : "更新中"}
              </dd>
            </div>
            <div>
              <dt>账户窗口剩余</dt>
              <dd>
                {!details
                  ? "更新中"
                  : accountWindow?.usedPercent == null
                    ? "来源未提供"
                    : `${Math.max(0, 100 - accountWindow.usedPercent).toFixed(0)}%${quotaExpired ? " · 过期" : ""}`}
              </dd>
            </div>
            <div>
              <dt>待查看提醒</dt>
              <dd>
                <button
                  className="text-button"
                  aria-label="查看今日未读提醒"
                  disabled={!canViewInbox}
                  onClick={() => void shortcuts.current?.openInbox()}
                >
                  {store.ready ? presentation?.unreadCount : "更新中"} · 查看今日未读
                </button>
              </dd>
            </div>
          </dl>
          <p className="subtle-note">
            {!details ? (
              store.detailsError ? (
                "小卡暂未更新，请稍后再试"
              ) : (
                "正在读取今日数据…"
              )
            ) : details.account?.collectedAt ? (
              <>
                账户{quotaExpired ? "历史快照（已过期）" : "采集"}{" "}
                {dateTime(details.account.collectedAt, true)}
              </>
            ) : (
              "账户：来源未提供"
            )}
            {accountWindow?.resetsAt != null && (
              <>
                <br />
                重置 {dateTime(accountWindow.resetsAt)} · 到点后等待实际重读
              </>
            )}
          </p>
          <p className="subtle-note">
            {!details ? "最近聊天待更新" : details.recentChat ? "当前主来源 · 今日最近记录" : "今日暂无可定位聊天"}
          </p>
          <button
            className="button secondary small full"
            disabled={!canReturnChat}
            onClick={() => void shortcuts.current?.openChat()}
          >
            返回最近聊天
            <Icon name="external" size={15} />
          </button>
          <button
            className="button primary small full"
            disabled={shortcutBusy}
            onClick={() => void open()}
          >
            打开主面板
            <Icon name="arrow" size={15} />
          </button>
        </section>
      )}
      {popover === "menu" && (
        <section
          ref={popoverElement}
          className="pet-popover pet-menu"
          aria-label="宠物菜单"
        >
          <CompanionMenu
            capabilities={
              companion.error ? undefined : companion.state?.capabilities
            }
          />
          {extensions.enabled && <PetPhysicsMenu physics={physics}
            onBeforeRequest={async () => await controller.current?.closeForAction() ?? false} />}
          {supportsAffection && PET_REACTIONS.map((reaction) => (
            <button key={reaction.id} onClick={() => affection(reaction.id)}>
              <Icon name={reaction.id === "affection" ? "heart" : "sparkles"} size={17} />
              {reaction.id === "affection" ? "摸摸小柚" : reaction.label}
            </button>
          ))}
          <button onClick={toggleCard}>
            <Icon name="grid" size={17} />
            打开统计小卡
          </button>
          <button onClick={() => void open()}>
            <Icon name="grid" size={17} />
            打开主面板
          </button>
          <button
            disabled={!store.ready || store.pending}
            onClick={() => {
              close();
              void store.save({ muted: !settings?.muted });
            }}
          >
            <Icon name="volume" size={17} />
            {settings?.muted ? "取消静音" : "开启静音"}
          </button>
          <button
            disabled={!store.ready || store.pending}
            onClick={() => {
              close();
              void store.save({
                doNotDisturb: !settings?.doNotDisturb,
              });
            }}
          >
            <Icon name="moon" size={17} />
            {settings?.doNotDisturb ? "关闭手动免打扰" : "开启手动免打扰"}
          </button>
          <TemporaryDoNotDisturbMenu settings={settings ?? {}} pending={!store.ready || store.pending}
            onRequest={(temporaryDoNotDisturbMinutes) => {
              close();
              void store.save({ temporaryDoNotDisturbMinutes });
            }} />
          <button
            onClick={() => {
              close();
              void store.save({ clickThrough: true });
            }}
          >
            <Icon name="eye" size={17} />
            开启整体穿透
          </button>
          <button
            onClick={() => {
              close();
              void store.resetPet();
            }}
          >
            <Icon name="cat" size={17} />
            一键找回宠物
          </button>
          <button
            onClick={() => {
              close();
              if (bridge.isDesktop)
                void invoke("set_pet_visibility", { visible: false });
            }}
          >
            <Icon name="pause" size={17} />
            隐藏角色（托盘恢复）
          </button>
          <button
            className="menu-danger"
            onClick={() => {
              close();
              if (bridge.isDesktop) void bridge.quitApp();
              else window.close();
            }}
          >
            <Icon name="close" size={17} />
            退出应用
          </button>
        </section>
      )}
      {store.toast && (
        <div className="pet-toast" role="status" style={{ userSelect: "text", overflowWrap: "anywhere" }}>
          {store.toast}
        </div>
      )}
    </div>
  );
}
