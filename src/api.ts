// 统一封装 Tauri invoke，集中类型与错误处理。
import { invoke } from "@tauri-apps/api/core";
import type {
  Account,
  AccountBilling,
  AppInfo,
  AppSettings,
  BalancePoint,
  CheckinResult,
  CheckinStatus,
  CreditLedger,
  DashboardSummary,
  DataItem,
  HeatCell,
  ModelInfo,
  OAuthPoll,
  OAuthStart,
  ProxyStatus,
  RefreshResult,
  RequestRow,
  Summary,
  TopItem,
  TravelOverview,
  TrendPoint,
} from "./types";

export const api = {
  // 通用 / 系统
  appInfo: () => invoke<AppInfo>("app_info"),
  getSettings: () => invoke<AppSettings>("get_settings"),
  saveSettings: (settings: AppSettings) =>
    invoke<void>("save_settings", { settings }),
  proxyStatus: () => invoke<ProxyStatus>("proxy_status"),

  // 账号
  listAccounts: () => invoke<Account[]>("list_accounts"),
  credentialStatus: (uid: string) =>
    invoke<boolean>("credential_status", { uid }),
  setAccountEnabled: (uid: string, enabled: boolean) =>
    invoke<void>("set_account_enabled", { uid, enabled }),
  setAccountTags: (uid: string, tags: string[]) =>
    invoke<void>("set_account_tags", { uid, tags }),
  deleteAccount: (uid: string) => invoke<void>("delete_account", { uid }),
  refreshQuota: (uids?: string[]) =>
    invoke<RefreshResult[]>("refresh_quota", { uids: uids ?? null }),
  creditLedger: (uid: string, days: number) =>
    invoke<CreditLedger>("credit_ledger", { uid, days }),

  // WorkBuddy 内置模型列表（实时）
  listModels: (uid?: string) =>
    invoke<ModelInfo[]>("list_models", { uid: uid ?? null }),

  // OAuth
  oauthBegin: () => invoke<OAuthStart>("oauth_begin"),
  oauthPoll: (sessionId: string) =>
    invoke<OAuthPoll>("oauth_poll", { sessionId }),

  // 签到
  checkinStatus: (uid: string) =>
    invoke<CheckinStatus>("checkin_status", { uid }),
  checkinAll: () => invoke<CheckinResult[]>("checkin_all"),
  checkinOne: (uid: string) => invoke<CheckinResult>("checkin_one", { uid }),

  // 猫猫旅行
  travelOverview: () => invoke<TravelOverview>("travel_overview"),
  travelDepart: (uid: string, locationId: number) =>
    invoke<unknown>("travel_depart", { uid, locationId }),
  travelClaim: (uid: string) => invoke<unknown>("travel_claim", { uid }),

  // 数据管理
  dataUids: () => invoke<string[]>("data_uids"),
  dataList: (category: string, uid?: string) =>
    invoke<DataItem[]>("data_list", { category, uid: uid ?? null }),
  dataRead: (item: DataItem) => invoke<string>("data_read", { item }),
  dataDelete: (items: DataItem[]) => invoke<number>("data_delete", { items }),
  dataExport: (item: DataItem, destDir: string) =>
    invoke<string>("data_export", { item, destDir }),
  dataBackup: (items: DataItem[], label: string, destDir?: string) =>
    invoke<string>("data_backup", { items, label, destDir: destDir || null }),
  dataMigrate: (fromUid: string, toUid: string, categories: string[]) =>
    invoke<number>("data_migrate", { fromUid, toUid, categories }),

  // 看板
  dashboardSummary: (fromMs: number, toMs: number) =>
    invoke<DashboardSummary>("dashboard_summary", { fromMs, toMs }),
  dashboardTrend: (fromMs: number, toMs: number, granularity: string) =>
    invoke<TrendPoint[]>("dashboard_trend", { fromMs, toMs, granularity }),
  dashboardTop: (
    fromMs: number,
    toMs: number,
    kind: "models" | "sessions" | "accounts",
    limit?: number,
  ) => invoke<TopItem[]>("dashboard_top", { fromMs, toMs, kind, limit }),
  dashboardHeatmap: (fromMs: number, toMs: number) =>
    invoke<HeatCell[]>("dashboard_heatmap", { fromMs, toMs }),
  dashboardBalances: () => invoke<BalancePoint[]>("dashboard_balances"),
  dashboardImportApp: (uid?: string) =>
    invoke<number>("dashboard_import_app", { uid: uid ?? null }),
  dashboardRequests: (fromMs: number, toMs: number, limit?: number) =>
    invoke<RequestRow[]>("dashboard_requests", {
      fromMs,
      toMs,
      limit: limit ?? null,
    }),
  dashboardAccountBilling: (fromMs: number, toMs: number) =>
    invoke<AccountBilling>("dashboard_account_billing", { fromMs, toMs }),
};

// 错误信息规整：后端 WmError 序列化为 {code, message}，也可能是字符串。
export function errText(e: unknown): string {
  if (e == null) return "未知错误";
  if (typeof e === "string") return e;
  const obj = e as Record<string, unknown>;
  if (typeof obj.message === "string") return obj.message;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

export type { Summary };
