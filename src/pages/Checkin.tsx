import { useEffect, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Col,
  Row,
  Space,
  Switch,
  Table,
  Tag,
  TimePicker,
  message,
} from "antd";
import dayjs from "dayjs";
import { GiftOutlined, ReloadOutlined } from "@ant-design/icons";
import { enable as enableAutoStart, disable as disableAutoStart, isEnabled as isAutoStartEnabled } from "@tauri-apps/plugin-autostart";
import { api, errText } from "../api";
import { useStore } from "../store";
import type { CheckinResult } from "../types";

export default function Checkin() {
  const accounts = useStore((s) => s.accounts);
  const settings = useStore((s) => s.settings);
  const saveSettings = useStore((s) => s.saveSettings);

  const [running, setRunning] = useState(false);
  const [results, setResults] = useState<Record<string, CheckinResult>>({});
  const [auto, setAuto] = useState(settings.checkin);
  const [autostart, setAutostart] = useState(false);
  const [autostartLoading, setAutostartLoading] = useState(false);

  useEffect(() => setAuto(settings.checkin), [settings.checkin]);
  useEffect(() => {
    isAutoStartEnabled().then(setAutostart).catch(() => {});
  }, []);

  const toggleAutostart = async (v: boolean) => {
    setAutostartLoading(true);
    try {
      if (v) await enableAutoStart();
      else await disableAutoStart();
      setAutostart(v);
      message.success(v ? "已开启开机自启" : "已关闭开机自启");
    } catch (e) {
      message.error(errText(e) || "设置失败");
    } finally {
      setAutostartLoading(false);
    }
  };

  const eligible = accounts.filter((a) => a.has_local_token);
  // 今日已签数：兼顾持久化的 today_checked_in 与本次刚签成功的结果（刚签到后 store 未重载也能即时反映）。
  const checkedToday = eligible.filter(
    (a) => a.today_checked_in || results[a.uid]?.ok
  ).length;

  const runAll = async () => {
    if (!eligible.length) {
      message.warning("没有可签到的账号（需先导入凭证）");
      return;
    }
    setRunning(true);
    try {
      const rs = await api.checkinAll();
      const map: Record<string, CheckinResult> = {};
      for (const r of rs) map[r.uid] = r;
      setResults(map);
      const ok = rs.filter((r) => r.ok && !r.already).length;
      message.success(`签到完成：新增 ${ok}，已签 ${rs.filter((r) => r.already).length}`);
    } catch (e) {
      message.error(errText(e));
    } finally {
      setRunning(false);
    }
  };

  const runOne = async (uid: string) => {
    try {
      const r = await api.checkinOne(uid);
      setResults((prev) => ({ ...prev, [uid]: r }));
      if (r.ok && !r.already) message.success(`${r.nickname || uid} 签到成功 +${r.reward}`);
      else if (r.already) message.info(`${r.nickname || uid} 今日已签到`);
      else message.warning(r.message || "签到失败");
    } catch (e) {
      message.error(errText(e));
    }
  };

  const saveAuto = async () => {
    try {
      await saveSettings({ ...settings, checkin: auto });
      message.success("自动签到设置已保存");
    } catch (e) {
      message.error(errText(e));
    }
  };

  return (
    <div className="wm-page">
      <Row gutter={16}>
        <Col xs={24} lg={16}>
          <Card
            size="small"
            title={
              <Space>
                <GiftOutlined />
                <span>
                  今日签到：{checkedToday}/{eligible.length}
                </span>
              </Space>
            }
            extra={
              <Button
                type="primary"
                loading={running}
                onClick={runAll}
                icon={<GiftOutlined />}
              >
                一键全部签到
              </Button>
            }
          >
            <Alert
              style={{ marginBottom: 12 }}
              type="info"
              showIcon
              message="Buddy 加油站活动：每日签到可得约 100 积分。签到后会自动刷新积分余额。"
            />
            <Table
              rowKey="uid"
              size="small"
              pagination={false}
              dataSource={accounts}
              columns={[
                { title: "昵称", dataIndex: "nickname", render: (t, r) => t || r.uid },
                {
                  title: "类型",
                  dataIndex: "account_type",
                  width: 80,
                  render: (t: string) => (
                    <Tag color={t === "enterprise" ? "gold" : "blue"}>
                      {t === "enterprise" ? "企业" : "个人"}
                    </Tag>
                  ),
                },
                {
                  title: "凭证",
                  width: 90,
                  render: (_, r) =>
                    r.has_local_token ? (
                      <Tag color="green">就绪</Tag>
                    ) : (
                      <Tag color="red">需授权</Tag>
                    ),
                },
                {
                  title: "今日",
                  width: 90,
                  render: (_, r) => {
                    const res = results[r.uid];
                    const done = res ? res.ok : r.today_checked_in;
                    return done ? (
                      <Tag color="purple">已签</Tag>
                    ) : (
                      <Tag>未签</Tag>
                    );
                  },
                },
                {
                  title: "连续",
                  width: 90,
                  render: (_, r) => {
                    const streak = results[r.uid]?.streak ?? r.streak_days;
                    return <span>{streak} 天</span>;
                  },
                },
                {
                  title: "结果",
                  render: (_, r) => {
                    const res = results[r.uid];
                    if (!res) return <span style={{ opacity: 0.4 }}>—</span>;
                    if (res.already) return <Tag>已签到过</Tag>;
                    if (res.ok) return <Tag color="green">+{res.reward}</Tag>;
                    return <Tag color="red">{res.message || "失败"}</Tag>;
                  },
                },
                {
                  title: "操作",
                  width: 100,
                  render: (_, r) => (
                    <Button
                      size="small"
                      disabled={!r.has_local_token}
                      onClick={() => runOne(r.uid)}
                    >
                      签到
                    </Button>
                  ),
                },
              ]}
            />
          </Card>
        </Col>

        <Col xs={24} lg={8}>
          <Card size="small" title="定时自动签到">
            <Space direction="vertical" style={{ width: "100%" }} size="middle">
              <Space>
                <Switch
                  checked={auto.autoEnabled}
                  onChange={(v) => setAuto({ ...auto, autoEnabled: v })}
                />
                <span>启用每日自动签到</span>
              </Space>
              <Space>
                <span>触发时间</span>
                <TimePicker
                  format="HH:mm"
                  value={dayjs(auto.dailyTime, "HH:mm")}
                  onChange={(v) =>
                    setAuto({ ...auto, dailyTime: v ? v.format("HH:mm") : "09:00" })
                  }
                  allowClear={false}
                />
              </Space>
              <Space>
                <Switch
                  checked={auto.notify}
                  onChange={(v) => setAuto({ ...auto, notify: v })}
                />
                <span>完成后系统通知</span>
              </Space>
              <Space>
                <Switch
                  checked={autostart}
                  loading={autostartLoading}
                  onChange={toggleAutostart}
                />
                <span>开机自动启动</span>
              </Space>
              <Alert
                type="warning"
                showIcon
                message="定时签到需保持应用运行，建议开启上方「开机自动启动」。错过时间启动后会补跑一次。"
              />
              <Button type="primary" onClick={saveAuto} icon={<ReloadOutlined />}>
                保存设置
              </Button>
            </Space>
          </Card>
        </Col>
      </Row>
    </div>
  );
}
