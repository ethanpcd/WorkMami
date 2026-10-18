import { useEffect, useMemo, useState } from "react";
import { ConfigProvider, Layout, Menu, Segmented, theme as antdTheme } from "antd";
import type { ThemeConfig } from "antd";
import zhCN from "antd/locale/zh_CN";
import {
  CloudServerOutlined,
  CompassOutlined,
  DashboardOutlined,
  DatabaseOutlined,
  GiftOutlined,
  TeamOutlined,
} from "@ant-design/icons";
import { useStore } from "./store";
import logo from "./assets/logo.png";
import wechatQr from "./assets/wechat-qr.jpg";
import Accounts from "./pages/Accounts";
import ProxySettings from "./pages/Proxy";
import DataPage from "./pages/Data";
import Checkin from "./pages/Checkin";
import Travel from "./pages/Travel";
import Dashboard from "./pages/Dashboard";

const { Sider, Content, Header } = Layout;

const MENU = [
  { key: "accounts", label: "账号管理", icon: <TeamOutlined /> },
  { key: "proxy", label: "代理设置", icon: <CloudServerOutlined /> },
  { key: "data", label: "数据管理", icon: <DatabaseOutlined /> },
  { key: "checkin", label: "签到中心", icon: <GiftOutlined /> },
  { key: "travel", label: "猫猫旅行", icon: <CompassOutlined /> },
  { key: "dashboard", label: "Token 看板", icon: <DashboardOutlined /> },
];

function useSystemDark() {
  const [sysDark, setSysDark] = useState(
    () => window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? true,
  );
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = (e: MediaQueryListEvent) => setSysDark(e.matches);
    mq.addEventListener?.("change", handler);
    return () => mq.removeEventListener?.("change", handler);
  }, []);
  return sysDark;
}

function SiderFooter({ isDark }: { isDark: boolean }) {
  const sub = isDark ? "#8a8a8a" : "#8c8c8c";
  const faint = isDark ? "#6a6a6a" : "#adb5bd";
  const divider = isDark ? "#26262a" : "#f0f0f0";
  return (
    <div
      style={{
        marginTop: "auto",
        padding: "14px 16px 16px",
        textAlign: "center",
        userSelect: "text",
      }}
    >
      <img
        src={wechatQr}
        alt="老马公众号二维码"
        style={{
          width: 104,
          height: 104,
          borderRadius: 8,
          display: "block",
          margin: "0 auto 6px",
          background: "#fff",
          padding: 4,
        }}
      />
      <div style={{ fontSize: 12, color: sub, marginBottom: 8 }}>
        扫一扫关注老马公众号
      </div>
      <div style={{ fontSize: 12, color: sub, lineHeight: 1.7 }}>
        <div>个人博客：www.majiabin.com</div>
      </div>
      <div
        style={{
          marginTop: 10,
          paddingTop: 10,
          borderTop: `1px solid ${divider}`,
          fontSize: 11,
          color: faint,
          lineHeight: 1.6,
        }}
      >
        本工具仅供个人学习研究使用
        <br />
        一切非法用途责任自负
      </div>
    </div>
  );
}

export default function App() {
  const [active, setActive] = useState("accounts");
  const init = useStore((s) => s.init);
  const themePref = useStore((s) => s.settings.theme);
  const setThemePref = useStore((s) => s.setThemePref);
  const sysDark = useSystemDark();

  useEffect(() => {
    void init();
  }, [init]);

  const isDark = themePref === "dark" || (themePref === "system" && sysDark);

  const theme = useMemo<ThemeConfig>(
    () => ({
      token: { colorPrimary: "#1677ff", borderRadius: 8 },
      algorithm: isDark ? antdTheme.darkAlgorithm : antdTheme.defaultAlgorithm,
    }),
    [isDark],
  );

  return (
    <ConfigProvider locale={zhCN} theme={theme}>
      <Layout style={{ height: "100vh" }}>
        <Sider width={240} theme={isDark ? "dark" : "light"}>
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              height: "100vh",
            }}
          >
          <div
            style={{
              height: 72,
              flexShrink: 0,
              display: "flex",
              alignItems: "center",
              gap: 10,
              padding: "0 16px",
              borderBottom: `1px solid ${isDark ? "#26262a" : "#f0f0f0"}`,
            }}
          >
            <img
              src={logo}
              alt="WorkMami"
              style={{
                width: 40,
                height: 40,
                borderRadius: 10,
                display: "block",
                boxShadow: "0 2px 10px rgba(236,72,153,0.4)",
              }}
            />
            <div style={{ lineHeight: 1.15 }}>
              <div
                style={{
                  fontSize: 20,
                  fontWeight: 800,
                  letterSpacing: 7,
                  marginRight: -7,
                  background: "linear-gradient(95deg,#ec4899 0%,#f472b6 55%,#fb7185 100%)",
                  WebkitBackgroundClip: "text",
                  backgroundClip: "text",
                  color: "transparent",
                }}
              >
                WorkMami
              </div>
              <div
                style={{
                  fontSize: 11,
                  color: isDark ? "#9a9a9a" : "#999",
                  marginTop: 2,
                  letterSpacing: 0,
                  whiteSpace: "nowrap",
                }}
              >
                WorkBuddy的妈咪稳稳接住你
              </div>
            </div>
          </div>
          <Menu
            theme={isDark ? "dark" : "light"}
            mode="inline"
            selectedKeys={[active]}
            items={MENU}
            onClick={(e) => setActive(e.key)}
          />
          <SiderFooter isDark={isDark} />
          </div>
        </Sider>
        <Layout>
          <Header
            style={{
              background: isDark ? "#141414" : "#fff",
              padding: "0 24px",
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              height: 56,
              lineHeight: "56px",
              borderBottom: `1px solid ${isDark ? "#303030" : "#f0f0f0"}`,
            }}
          >
            <span style={{ fontSize: 15, fontWeight: 600 }}>
              {MENU.find((m) => m.key === active)?.label}
            </span>
            <Segmented
              value={themePref}
              onChange={(v) => setThemePref(v as string)}
              options={[
                { label: "跟随系统", value: "system" },
                { label: "暗色", value: "dark" },
                { label: "亮色", value: "light" },
              ]}
            />
          </Header>
          <Content style={{ background: isDark ? "#000" : "#f5f5f5" }}>
            {active === "accounts" && <Accounts />}
            {active === "proxy" && <ProxySettings />}
            {active === "data" && <DataPage />}
            {active === "checkin" && <Checkin />}
            {active === "travel" && <Travel />}
            {active === "dashboard" && <Dashboard />}
          </Content>
        </Layout>
      </Layout>
    </ConfigProvider>
  );
}
