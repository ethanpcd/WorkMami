import { useCallback, useEffect, useRef, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Col,
  Row,
  Select,
  Space,
  Switch,
  Tag,
  Typography,
  message,
} from "antd";
import {
  CompassOutlined,
  GiftOutlined,
  ReloadOutlined,
} from "@ant-design/icons";
import { api, errText } from "../api";
import { useStore } from "../store";
import type { TravelOverview, TravelStatus, TravelLocation, TravelRecord } from "../types";

const { Text, Title } = Typography;

function formatCountdown(arriveAt: number, serverNow: number): string {
  // 用本地时间近似（serverNow 仅用于校准偏移）
  const offsetSec = Math.floor(Date.now() / 1000) - serverNow;
  const remainSec = arriveAt - (Math.floor(Date.now() / 1000) - offsetSec);
  if (remainSec <= 0) return "已到站";
  const h = Math.floor(remainSec / 3600);
  const m = Math.floor((remainSec % 3600) / 60);
  const s = remainSec % 60;
  if (h > 0) return `${h}时${m}分${s}秒`;
  if (m > 0) return `${m}分${s}秒`;
  return `${s}秒`;
}

function statusTag(state: string) {
  switch (state) {
    case "traveling":
      return <Tag color="processing">旅行中</Tag>;
    case "arrived":
      return <Tag color="success">可领取</Tag>;
    default:
      return <Tag>待出行</Tag>;
  }
}

// unix 秒 -> 本地“月-日 时:分”绝对时刻，让用户知道具体几点出发/到站。
function fmtClock(unixSec: number): string {
  if (!unixSec) return "—";
  const d = new Date(unixSec * 1000);
  return d.toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

const POLICY_LABEL: Record<string, string> = {
  longest: "最长时间",
  shortest: "最短时间",
  rotate: "轮换",
};

// 与服务端 travel_date 对齐的本地日期串 YYYY-MM-DD。
function todayStr(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

// 旅行账单：直接取服务端台账，逐条列出“派出时间 / 领取时间 / 赚多少 / 是否到账”，
// 让用户一目了然地看到上次什么时候派猫出去、奖励又是什么时候领到手。
// 今天有记录则按今日逐条展示；今天没记录但历史有→回落到“上次旅行”单条（跨天也能看到时刻）。
function TodayLedger({ records }: { records?: TravelRecord[] }) {
  const all = records || [];
  const today = todayStr();
  const rows = all.filter((r) => r.travelDate === today);
  const total = rows.reduce((s, r) => s + r.rewardCredit, 0);
  const unclaimed = rows.filter((r) => !r.claimedAt).length;
  const last = all[0]; // 服务端按 travel_date 倒序，首条即最近一次已完成旅行
  const shown = rows.length ? rows : last ? [last] : [];
  return (
    <div style={{ marginTop: 12, paddingTop: 10, borderTop: "1px dashed rgba(128,128,128,.3)" }}>
      <Text strong style={{ fontSize: 13 }}>
        {rows.length ? (
          <>
            今日旅行账单：{rows.length} 次 · 共 +{total} 分
            {unclaimed > 0 && (
              <Tag color="warning" style={{ marginLeft: 8 }}>
                {unclaimed} 笔待领取
              </Tag>
            )}
          </>
        ) : (
          <>上次旅行</>
        )}
      </Text>
      {shown.length === 0 ? (
        <Text type="secondary" style={{ display: "block", marginTop: 4, fontSize: 12 }}>
          还没有任何旅行记录。
        </Text>
      ) : (
        <div style={{ marginTop: 6 }}>
          {shown.map((r) => (
            <div key={r.recordId} style={{ padding: "2px 0" }}>
              <div style={{ display: "flex", justifyContent: "space-between", fontSize: 12 }}>
                <Text style={{ fontSize: 12 }}>
                  {r.travelDate !== today ? `${r.travelDate} · ` : ""}
                  {r.locationName}
                </Text>
                <span>
                  <Text strong style={{ fontSize: 12 }}>+{r.rewardCredit} 分</Text>
                  {r.claimedAt ? (
                    <Tag color="success" style={{ marginLeft: 6, fontSize: 11, lineHeight: "18px", padding: "0 4px" }}>
                      已到账
                    </Tag>
                  ) : (
                    <Tag color="warning" style={{ marginLeft: 6, fontSize: 11, lineHeight: "18px", padding: "0 4px" }}>
                      待领取
                    </Tag>
                  )}
                </span>
              </div>
              <div style={{ fontSize: 11, opacity: 0.65 }}>
                派出于 {fmtClock(r.departAt)} ·{" "}
                {r.claimedAt ? `奖励领于 ${fmtClock(r.claimedAt)}` : "奖励尚未领取"}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export default function Travel() {
  const [data, setData] = useState<TravelOverview | null>(null);
  const [loading, setLoading] = useState(false);
  const [, setTick] = useState(0); // 驱动倒计时刷新
  const timerRef = useRef<ReturnType<typeof setInterval>>();
  const settings = useStore((s) => s.settings);
  const saveSettings = useStore((s) => s.saveSettings);

  const load = useCallback(async (silent = false) => {
    if (!silent) setLoading(true);
    try {
      const res = await api.travelOverview();
      setData(res);
    } catch (e) {
      if (!silent) message.error(errText(e));
    } finally {
      if (!silent) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
    // 每秒刷新倒计时（本地，不走网）；每 5 分钟静默重拉一次状态，避免高频调用官方接口；后台自动派/领为小时级，5 分钟足够。
    timerRef.current = setInterval(() => setTick((t) => t + 1), 1000);
    const refresh = setInterval(() => void load(true), 300000);
    return () => {
      clearInterval(timerRef.current);
      clearInterval(refresh);
    };
  }, [load]);

  const handleDepart = async (uid: string, locationId: number) => {
    try {
      await api.travelDepart(uid, locationId);
      message.success("已派猫出发！");
      void load();
    } catch (e) {
      message.error(errText(e));
    }
  };

  const handleClaim = async (uid: string) => {
    try {
      await api.travelClaim(uid);
      message.success("奖励已领取！");
      void load();
    } catch (e) {
      message.error(errText(e));
    }
  };

  const toggleAuto = async (checked: boolean) => {
    const s = { ...settings, travel: { ...settings.travel, autoEnabled: checked } };
    await saveSettings(s);
  };

  const changePolicy = async (v: string) => {
    const s = { ...settings, travel: { ...settings.travel, locationPolicy: v } };
    await saveSettings(s);
  };

  if (!data) {
    return (
      <div style={{ padding: 24 }}>
        <Card loading={loading}>加载中...</Card>
      </div>
    );
  }

  return (
    <div style={{ padding: 24, overflow: "auto", height: "100%" }}>
      {/* 顶部设置区 */}
      <Card size="small" style={{ marginBottom: 16 }}>
        <Row gutter={24} align="middle">
          <Col>
            <Space>
              <Text strong>自动托管</Text>
              <Switch
                checked={settings.travel.autoEnabled}
                onChange={toggleAuto}
                checkedChildren="开"
                unCheckedChildren="关"
              />
            </Space>
          </Col>
          <Col>
            <Space>
              <Text strong>地点策略</Text>
              <Select
                value={settings.travel.locationPolicy}
                onChange={changePolicy}
                style={{ width: 120 }}
                options={[
                  { label: "最长时间", value: "longest" },
                  { label: "最短时间", value: "shortest" },
                  { label: "轮换", value: "rotate" },
                ]}
              />
            </Space>
          </Col>
          <Col flex="auto" style={{ textAlign: "right" }}>
            <Button icon={<ReloadOutlined />} onClick={() => load()} loading={loading}>
              刷新
            </Button>
          </Col>
        </Row>
      </Card>

      {settings.travel.autoEnabled && (
        <Alert
          type="info"
          showIcon
          style={{ marginBottom: 16 }}
          message="自动托管已开启：后台每 60 分钟自动巡检一次"
          description={
            <span>
              猫猫在家（空闲）时，会在下一次巡检（最迟约 60 分钟）按“{POLICY_LABEL[settings.travel.locationPolicy] || settings.travel.locationPolicy}”策略自动出发；旅行到站后也会在下一次巡检自动领取奖励，无需手动操作。一天只能派一次猫，一旦连续确认今日已达上限，后台当日会停止巡检以免高频调用官方接口触发风控（次日自动恢复）。仅在本客户端运行期间生效（页面每 5 分钟自动刷新，也可手动刷新）。
            </span>
          }
        />
      )}

      {/* 各账号旅行卡片 */}
      <Row gutter={[16, 16]}>
        {data.accounts.map((item) => {
          const st = item.status as TravelStatus;
          const isError = "error" in item.status;
          return (
            <Col key={item.uid} xs={24} md={12} lg={8}>
              <Card
                title={
                  <Space>
                    <CompassOutlined />
                    <span>{item.nickname || item.uid.slice(0, 8)}</span>
                  </Space>
                }
                extra={isError ? <Tag color="error">查询失败</Tag> : statusTag(st.state)}
                size="small"
              >
                {isError ? (
                  <Text type="danger">{(item.status as { error: string }).error}</Text>
                ) : (
                  <>
                    {st.state === "traveling" && (
                      <>
                        <Title level={5} style={{ margin: "8px 0" }}>
                          {st.locationName || "旅行中"}
                        </Title>
                        <Text type="secondary" style={{ display: "block" }}>
                          出发时刻：{fmtClock(st.departAt)}
                        </Text>
                        <Text type="secondary" style={{ display: "block" }}>
                          预计到达：{fmtClock(st.arriveAt)}
                        </Text>
                        <Text strong style={{ display: "block", marginTop: 4 }}>
                          还有 {formatCountdown(st.arriveAt, st.serverNow)} 到站
                        </Text>
                      </>
                    )}
                    {st.state === "arrived" && (
                      <>
                        <Title level={5} style={{ margin: "8px 0" }}>
                          {st.locationName || "已到达"}
                        </Title>
                        <Text type="secondary" style={{ display: "block", marginBottom: 4 }}>
                          到站时刻：{fmtClock(st.arriveAt)}
                        </Text>
                        {settings.travel.autoEnabled && (
                          <Text type="secondary" style={{ display: "block", marginBottom: 8 }}>
                            自动托管下将在下一次巡检（≤ 60 分钟）自动领取，也可点下方按钮立即领取。
                          </Text>
                        )}
                        <Button
                          type="primary"
                          icon={<GiftOutlined />}
                          onClick={() => handleClaim(item.uid)}
                        >
                          立即领取
                        </Button>
                      </>
                    )}
                    {st.state === "idle" && (
                      <>
                        {st.dailyLimitReached ? (
                          <Alert
                            type="warning"
                            showIcon
                            style={{ marginBottom: 12 }}
                            message="今日旅行次数已达上限"
                            description="服务端判定该账号今天的旅行次数已用完，自动托管会暂停派猫，明日额度刷新后恢复出发。今天仍可尝试手动出发（以服务端为准）。"
                          />
                        ) : (
                          <Text type="secondary" style={{ display: "block", marginBottom: 12 }}>
                            {settings.travel.autoEnabled
                              ? `猫猫在家。自动托管已开，最迟约 60 分钟内会按「${POLICY_LABEL[settings.travel.locationPolicy] || settings.travel.locationPolicy}」策略自动出发。`
                              : "猫猫在家休息，可手动选目的地出发，或开启自动托管。"}
                          </Text>
                        )}
                        <Select
                          placeholder={settings.travel.autoEnabled ? "手动立即出发（可选）" : "选择目的地出发"}
                          style={{ width: "100%", marginBottom: 8 }}
                          onChange={(v: number) => handleDepart(item.uid, v)}
                          options={data.config.map((loc: TravelLocation) => ({
                            label: `${loc.name} (${loc.durationMinHours}-${loc.durationMaxHours}h)`,
                            value: loc.id,
                          }))}
                        />
                      </>
                    )}
                    <TodayLedger records={item.records} />
                  </>
                )}
              </Card>
            </Col>
          );
        })}
      </Row>
    </div>
  );
}
