// 全局状态：账号列表、设置、主题、反代状态。
import { create } from "zustand";
import { api, errText } from "./api";
import type { Account, AppInfo, AppSettings, ProxyStatus } from "./types";

export const defaultSettings: AppSettings = {
  proxy: {
    enabled: false,
    host: "127.0.0.1",
    port: 8765,
    apiKey: "",
    strategy: "smart",
    contextWindow: 128000,
    reasoningEffort: "medium",
    defaultModel: "wb-balanced",
    stickyTtlMin: 30,
    maxRetries: 3,
  },
  checkin: { autoEnabled: false, dailyTime: "09:00", notify: true },
  travel: { autoEnabled: true, locationPolicy: "longest" },
  theme: "light",
  autostart: false,
};

interface Store {
  info?: AppInfo;
  settings: AppSettings;
  accounts: Account[];
  proxy?: ProxyStatus;
  loading: boolean;

  init: () => Promise<void>;
  reloadAccounts: () => Promise<void>;
  reloadProxy: () => Promise<void>;
  saveSettings: (s: AppSettings) => Promise<void>;
  setThemePref: (t: string) => void;
}

export const useStore = create<Store>((set, get) => ({
  settings: defaultSettings,
  accounts: [],
  loading: false,

  init: async () => {
    set({ loading: true });
    try {
      const [info, settings, accounts, proxy] = await Promise.all([
        api.appInfo(),
        api.getSettings(),
        api.listAccounts(),
        api.proxyStatus(),
      ]);
      set({ info, settings, accounts, proxy });
    } catch (e) {
      console.error("初始化失败:", errText(e));
    } finally {
      set({ loading: false });
    }
  },

  reloadAccounts: async () => {
    try {
      set({ accounts: await api.listAccounts() });
    } catch (e) {
      console.error(errText(e));
    }
  },

  reloadProxy: async () => {
    try {
      set({ proxy: await api.proxyStatus() });
    } catch (e) {
      console.error(errText(e));
    }
  },

  saveSettings: async (s: AppSettings) => {
    await api.saveSettings(s);
    set({ settings: s });
    await get().reloadProxy();
  },

  setThemePref: (t: string) => {
    const s = { ...get().settings, theme: t };
    set({ settings: s });
    void api.saveSettings(s);
  },
}));
