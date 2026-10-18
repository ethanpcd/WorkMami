// 与 Rust 后端 serde 输出对齐的 TS 类型。
// 注意：Account/QuotaSnapshot/AuthTokens 为 snake_case，其余带 rename_all 的为 camelCase。

export interface QuotaResource {
  package_code: string;
  name: string;
  used: number;
  total: number;
  remaining: number;
  expire_at?: number | null; // 毫秒；null = 长期有效
  expired: boolean;
  expiring_soon: boolean;
}

export interface QuotaSnapshot {
  used: number;
  total: number;
  remaining: number;
  plan: string;
  soonest_expire_at?: number | null; // 毫秒
  expiring_soon: boolean;
  resources: QuotaResource[];
  updated_at: string;
}

export interface Account {
  uid: string;
  uin: string;
  account_type: string;
  nickname: string;
  source: string; // scan / oauth / manual
  tags: string[];
  enabled: boolean;
  quota?: QuotaSnapshot | null;
  last_used_at?: string | null;
  today_checked_in: boolean;
  streak_days: number;
  has_local_token: boolean;
  created_at: string;
}

export interface AuthTokens {
  access_token: string;
  refresh_token: string;
  token_type: string;
  expires_at?: number | null;
  refresh_expires_at?: number | null;
  scope: string;
}

export interface RefreshResult {
  uid: string;
  nickname: string;
  ok: boolean;
  quota?: QuotaSnapshot | null;
  error?: string | null;
}

// 积分清单（后端 camelCase）
export interface CreditReward {
  name: string;
  reason: string;
  used: number;
  total: number;
  remaining: number;
  expireAt?: number | null;
  status: string; // 使用中 / 已用完 / 已过期
}

export interface CreditUsageRow {
  time: string;
  ts: number;
  requestId: string;
  credits: number;
  model: string;
  client: string;
}

export interface CreditLedger {
  rewards: CreditReward[];
  rewardCount: number;
  rewardRemaining: number;
  usage: CreditUsageRow[];
  usageTotal: number;
  days: number;
}

export interface CheckinResult {
  uid: string;
  nickname: string;
  ok: boolean;
  reward: number;
  already: boolean;
  streak: number;
  message: string;
}

export type Strategy = "round-robin" | "smart" | "drain";

export interface ProxySettings {
  enabled: boolean;
  host: string;
  port: number;
  apiKey: string;
  strategy: Strategy;
  contextWindow: number;
  reasoningEffort: string; // low/medium/high/none
  defaultModel: string;
  stickyTtlMin: number;
  maxRetries: number;
}

export interface CheckinSettings {
  autoEnabled: boolean;
  dailyTime: string; // "HH:MM"
  notify: boolean;
}

export interface TravelSettings {
  autoEnabled: boolean;
  locationPolicy: string; // "longest" | "shortest" | "rotate"
}

export interface AppSettings {
  proxy: ProxySettings;
  checkin: CheckinSettings;
  travel: TravelSettings;
  theme: string; // dark / light / system
  autostart: boolean;
}

export interface ProxyStatus {
  running: boolean;
  enabled: boolean;
  host: string;
  port: number;
  baseUrl: string;
}

export interface AppInfo {
  platform: string;
  version: string;
  paths: {
    workmamiRoot: string;
    workbuddyHome: string;
    desktopInfo?: string | null;
  };
}

export interface DataItem {
  id: string;
  category: string; // sessions/memory/agent-config/skills/artifacts
  name: string;
  path: string;
  size: number;
  modified: number; // ms
  meta?: Record<string, unknown> | null;
}

export interface Summary {
  totalTokens: number;
  promptTokens: number;
  completionTokens: number;
  reasoningTokens: number;
  cachedTokens: number;
  credits: number;
  requests: number;
  success: number;
  successRate: number;
  avgTokensPerReq: number;
  cacheHitRate: number;
  avgLatencyMs: number;
}

export interface DashboardSummary {
  current: Summary;
  previous: Summary;
}

export interface TrendPoint {
  bucket: string;
  tokens: number;
  credits: number;
  requests: number;
}

export interface TopItem {
  key: string;
  label: string;
  tokens: number;
  credits: number;
  requests: number;
}

export interface HeatCell {
  weekday: number; // 0=Mon
  hour: number; // 0-23
  tokens: number;
}

export interface BalancePoint {
  ts: number;
  uid: string;
  remaining: number;
}

// 请求流水（本工具代理/导入的每条记录）
export interface RequestRow {
  ts: number;
  uid: string;
  model: string;
  source: string; // proxy / app
  totalTokens: number;
  credits: number;
  latencyMs: number;
  status: number;
  ok: boolean;
}

// 各账号真实扣费（来自官方账单）
export interface AccountBillingItem {
  uid: string;
  nickname: string;
  credits: number;
  requests: number;
}

export interface AccountBilling {
  days: number;
  total: number;
  items: AccountBillingItem[];
}

// WorkBuddy 内置模型（实时拉取，字段以官方客户端为准）
export interface ModelInfo {
  id: string;
  name: string;
  contextWindow: number; // 上下文窗口（输入上限）
  maxOutputTokens: number;
  supportsToolCall: boolean;
  supportsImages: boolean;
  supportsReasoning: boolean;
  defaultEffort: string; // low/medium/high/none，可能为空
  description: string;
}

export interface OAuthStart {
  sessionId: string;
}

export interface OAuthPoll {
  status: "pending" | "authorized" | "expired";
  account?: Account;
}

export interface CheckinStatus {
  checkedIn: boolean;
  streakDays: number;
  status: unknown;
  activity: unknown;
}

// ---------- 猫猫旅行 ----------
export interface TravelLocation {
  id: number;
  name: string;
  durationMinHours: number;
  durationMaxHours: number;
}

export interface TravelStatus {
  state: string; // idle / traveling / arrived
  locationName: string;
  departAt: number; // unix 秒
  arriveAt: number; // unix 秒
  serverNow: number; // unix 秒
  /** 服务端判定今日旅行次数已达上限：state 仍为 idle 但再派猫会被拒 */
  dailyLimitReached: boolean;
}

export interface TravelAccountStatus {
  uid: string;
  nickname: string;
  status: TravelStatus | { error: string };
  records?: TravelRecord[];
}

// 单次旅行台账（服务端真实记录）
export interface TravelRecord {
  recordId: number;
  locationName: string;
  travelDate: string; // YYYY-MM-DD
  departAt: number; // unix 秒
  arriveAt: number;
  claimedAt: number; // 0 = 尚未领取
  rewardCredit: number;
}

export interface TravelOverview {
  config: TravelLocation[];
  accounts: TravelAccountStatus[];
  autoEnabled: boolean;
  locationPolicy: string;
}
