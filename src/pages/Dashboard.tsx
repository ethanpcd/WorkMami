import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Button,
  Card,
  Col,
  DatePicker,
  Radio,
  Row,
  Segmented,
  Space,
  Statistic,
  Table,
  Tabs,
  Tag,
  message,
} from "antd";
import dayjs, { Dayjs } from "dayjs";
import ReactECharts from "echarts-for-react";
import { ImportOutlined, ReloadOutlined } from "@ant-design/icons";
import { api, errText } from "../api";
import { useStore } from "../store";
import type {
  AccountBilling,
  AccountBillingItem,
  BalancePoint,
  DashboardSummary,
  HeatCell,
  RequestRow,
  TopItem,
  TrendPoint,
} from "../types";

const { RangePicker } = DatePicker;

type RangeKey = "today" | "7d" | "30d" | "custom";

function rangeMs(key: RangeKey, custom?: [Dayjs, Dayjs]): [number, number] {
  const now = dayjs();
  if (key === "today") return [now.startOf("day").valueOf(), now.endOf("day").valueOf()];
  if (key === "7d")
    return [now.subtract(6, "day").startOf("day").valueOf(), now.endOf("day").valueOf()];
  if (key === "30d")
    return [now.subtract(29, "day").startOf("day").valueOf(), now.endOf("day").valueOf()];
  if (custom) return [custom[0].startOf("day").valueOf(), custom[1].endOf("day").valueOf()];
  return [now.subtract(29, "day").startOf("day").valueOf(), now.endOf("day").valueOf()];
}

function granularityFor(key: RangeKey): string {
  if (key === "today") return "hour";
  return "day";
}

// 积分显示按数值自适应精度：固定 1 位小数会把 0.02 四舍五入成 "0.0"，
// 导致看板“估算积分”看似为 0（而流水用 2 位能显示 0.02），两处对不上让用户误以为数据丢了。
function fmtCredits(v: number): string {
  if (!v) return "0";
  const a = Math.abs(v);
  const p = a >= 100 ? 0 : a >= 10 ? 1 : 2;
  return v.toFixed(p);
}

export default function Dashboard() {
  const isDark = useStore((s) => s.settings.theme) !== "light";
  const accounts = useStore((s) => s.accounts);
  const [range, setRange] = useState<RangeKey>("30d");
  const [custom, setCustom] = useState<[Dayjs, Dayjs] | undefined>();
  const [loading, setLoading] = useState(false);
  const [granularity, setGranularity] = useState<string>("day");

  // uid → 昵称映射：看板所有账号维度都用昵称替代 uid 尾号，一眼认得出。
  const nameByUid = useMemo(() => {
    const m: Record<string, string> = {};
    for (const a of accounts) m[a.uid] = a.nickname || `…${a.uid.slice(-6)}`;
    return m;
  }, [accounts]);

  const [summary, setSummary] = useState<DashboardSummary | null>(null);
  const [trend, setTrend] = useState<TrendPoint[]>([]);
  const [topModels, setTopModels] = useState<TopItem[]>([]);
  const [topSessions, setTopSessions] = useState<TopItem[]>([]);
  const [topAccounts, setTopAccounts] = useState<TopItem[]>([]);
  const [heat, setHeat] = useState<HeatCell[]>([]);
  const [balances, setBalances] = useState<BalancePoint[]>([]);
  const [requests, setRequests] = useState<RequestRow[]>([]);
  const [billing, setBilling] = useState<AccountBilling | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    const [from, to] = rangeMs(range, custom);
    try {
      const [sm, tr, m, s, a, h, b, rq, bl] = await Promise.all([
        api.dashboardSummary(from, to),
        api.dashboardTrend(from, to, granularity),
        api.dashboardTop(from, to, "models", 10),
        api.dashboardTop(from, to, "sessions", 10),
        api.dashboardTop(from, to, "accounts", 10),
        api.dashboardHeatmap(from, to),
        api.dashboardBalances(),
        api.dashboardRequests(from, to, 300),
        api.dashboardAccountBilling(from, to),
      ]);
      setSummary(sm);
      setTrend(tr);
      setTopModels(m);
      setTopSessions(s);
      setTopAccounts(a);
      setHeat(h);
      setBalances(b);
      setRequests(rq);
      setBilling(bl);
    } catch (e) {
      message.error(errText(e));
    } finally {
      setLoading(false);
    }
  }, [range, custom, granularity]);

  useEffect(() => {
    void load();
  }, [load]);

  const importApp = async () => {
    try {
      const n = await api.dashboardImportApp();
      message.success(`已导入 App 内用量 ${n} 条`);
      await load();
    } catch (e) {
      message.error(errText(e));
    }
  };

  const chartTextColor = isDark ? "#ddd" : "#333";
  const cur = summary?.current;
  const prev = summary?.previous;

  const trendOption = useMemo(
    () => ({
      textStyle: { color: chartTextColor },
      tooltip: { trigger: "axis" },
      legend: { data: ["Tokens", "积分", "请求数"], textStyle: { color: chartTextColor } },
      grid: { left: 50, right: 50, top: 40, bottom: 30 },
      xAxis: {
        type: "category",
        data: trend.map((t) => t.bucket),
        axisLabel: { color: chartTextColor },
      },
      yAxis: [
        { type: "value", name: "Tokens", axisLabel: { color: chartTextColor } },
        { type: "value", name: "积分", axisLabel: { color: chartTextColor } },
      ],
      series: [
        {
          name: "Tokens",
          type: "bar",
          data: trend.map((t) => t.tokens),
          itemStyle: { color: "#1677ff" },
        },
        {
          name: "积分",
          type: "line",
          yAxisIndex: 1,
          smooth: true,
          data: trend.map((t) => t.credits),
          itemStyle: { color: "#faad14" },
        },
        {
          name: "请求数",
          type: "line",
          smooth: true,
          data: trend.map((t) => t.requests),
          itemStyle: { color: "#52c41a" },
        },
      ],
    }),
    [trend, chartTextColor],
  );

  const heatOption = useMemo(() => {
    const days = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
    const hours = Array.from({ length: 24 }, (_, i) => `${i}`);
    const data = heat.map((c) => [c.hour, c.weekday, c.tokens]);
    const max = Math.max(1, ...heat.map((c) => c.tokens));
    return {
      textStyle: { color: chartTextColor },
      tooltip: { position: "top" },
      grid: { left: 50, right: 20, top: 20, bottom: 40 },
      xAxis: { type: "category", data: hours, splitArea: { show: true } },
      yAxis: { type: "category", data: days, splitArea: { show: true } },
      visualMap: {
        min: 0,
        max,
        calculable: true,
        orient: "horizontal",
        left: "center",
        bottom: 0,
        inRange: { color: isDark ? ["#102339", "#1677ff", "#69b1ff"] : ["#e6f4ff", "#1677ff", "#0958d9"] },
      },
      series: [
        {
          type: "heatmap",
          data,
          label: { show: false },
        },
      ],
    };
  }, [heat, chartTextColor, isDark]);

  const balanceOption = useMemo(() => {
    const byUid = new Map<string, BalancePoint[]>();
    for (const b of balances) {
      if (!byUid.has(b.uid)) byUid.set(b.uid, []);
      byUid.get(b.uid)!.push(b);
    }
    const series = [...byUid.entries()].map(([uid, pts]) => ({
      name: nameByUid[uid] || uid.slice(-6),
      type: "line" as const,
      step: "end" as const,
      showSymbol: true,
      symbolSize: 6,
      // 时间轴必须用数值时间戳（ms）作为 x：之前喂格式化字符串 "MM-DD HH:mm" 无法被
      // echarts time 轴解析，导致每个点变 Invalid Date 被丢弃、整条线画不出来。
      data: pts
        .sort((a, b) => a.ts - b.ts)
        .map((p) => [p.ts, Math.round(p.remaining)]),
    }));
    return {
      textStyle: { color: chartTextColor },
      tooltip: {
        trigger: "axis",
        formatter: (params: any) => {
          const arr = Array.isArray(params) ? params : [params];
          const head = arr[0]?.axisValue;
          const lines = arr
            .map((it: any) => `${it.marker}${it.seriesName}: ${it.data?.[1] ?? "-"}`)
            .join("<br/>");
          return `${head ? dayjs(head).format("MM-DD HH:mm") : ""}<br/>${lines}`;
        },
      },
      legend: { textStyle: { color: chartTextColor }, type: "scroll" as const },
      grid: { left: 50, right: 60, top: 40, bottom: 40 },
      xAxis: {
        type: "time",
        axisLabel: { color: chartTextColor, hideOverlap: true, formatter: (val: number) => dayjs(val).format("MM-DD HH:mm") },
      },
      yAxis: { type: "value", name: "余额", scale: true, axisLabel: { color: chartTextColor } },
      series,
    };
  }, [balances, chartTextColor, nameByUid]);

  const delta = (c?: number, p?: number) => {
    if (c == null || p == null || p === 0) return undefined;
    return (((c - p) / p) * 100).toFixed(1) + "%";
  };

  return (
    <div className="wm-page">
      <div className="wm-toolbar">
        <Radio.Group
          value={range}
          onChange={(e) => {
            const r = e.target.value as RangeKey;
            setRange(r);
            setGranularity(granularityFor(r));
          }}
          optionType="button"
          buttonStyle="solid"
          options={[
            { label: "今日", value: "today" },
            { label: "近7日", value: "7d" },
            { label: "近30日", value: "30d" },
            { label: "自定义", value: "custom" },
          ]}
        />
        {range === "custom" && (
          <RangePicker
            onChange={(v) => v && setCustom([v[0] as Dayjs, v[1] as Dayjs])}
          />
        )}
        <Segmented
          value={granularity}
          onChange={(v) => setGranularity(v as string)}
          options={[
            { label: "按小时", value: "hour" },
            { label: "按天", value: "day" },
            { label: "按周", value: "week" },
            { label: "按月", value: "month" },
          ]}
        />
        <Space style={{ marginLeft: "auto" }}>
          <Button icon={<ImportOutlined />} onClick={importApp}>
            导入 App 用量
          </Button>
          <Button icon={<ReloadOutlined />} loading={loading} onClick={load}>
            刷新
          </Button>
        </Space>
      </div>

      <Row gutter={12} style={{ marginBottom: 16 }}>
        <Col span={4}>
          <Card size="small">
            <Statistic title="总 Tokens" value={cur?.totalTokens ?? 0} />
            <div style={{ fontSize: 12, opacity: 0.6 }}>
              环比 {delta(cur?.totalTokens, prev?.totalTokens) ?? "—"}
            </div>
          </Card>
        </Col>
        <Col span={4}>
          <Card size="small">
            <Statistic
              title="估算积分"
              value={fmtCredits(cur?.credits ?? 0)}
              suffix="分"
            />
          </Card>
        </Col>
        <Col span={4}>
          <Card size="small">
            <Statistic title="请求次数" value={cur?.requests ?? 0} />
            <div style={{ fontSize: 12, opacity: 0.6 }}>
              成功率 {(cur?.successRate ?? 0).toFixed(1)}%
            </div>
          </Card>
        </Col>
        <Col span={4}>
          <Card size="small">
            <Statistic
              title="缓存命中"
              value={cur?.cacheHitRate ?? 0}
              precision={1}
              suffix="%"
            />
          </Card>
        </Col>
        <Col span={4}>
          <Card size="small">
            <Statistic
              title="均次 Tokens"
              value={cur?.avgTokensPerReq ?? 0}
              precision={0}
            />
          </Card>
        </Col>
        <Col span={4}>
          <Card size="small">
            <Statistic
              title="平均耗时"
              value={cur?.avgLatencyMs ?? 0}
              precision={0}
              suffix="ms"
            />
          </Card>
        </Col>
      </Row>

      <Card size="small" title="消耗趋势" style={{ marginBottom: 16 }}>
        <ReactECharts option={trendOption} style={{ height: 280 }} notMerge />
      </Card>

      <Row gutter={16} style={{ marginBottom: 16 }}>
        <Col xs={24} lg={12}>
          <Card size="small" title="Top 消耗榜">
            <Tabs
              size="small"
              items={[
                {
                  key: "models",
                  label: "模型",
                  children: <TopTable data={topModels} />,
                },
                {
                  key: "sessions",
                  label: "会话",
                  children: <TopTable data={topSessions} />,
                },
                {
                  key: "accounts",
                  label: "账号",
                  children: (
                    <TopTable
                      data={topAccounts.map((x) => ({
                        ...x,
                        label: nameByUid[x.key] || x.label,
                      }))}
                    />
                  ),
                },
              ]}
            />
          </Card>
        </Col>
        <Col xs={24} lg={12}>
          <Card size="small" title="时段热力图 (近30日)">
            <ReactECharts option={heatOption} style={{ height: 260 }} notMerge />
          </Card>
        </Col>
      </Row>

      <Card
        size="small"
        title="各账号真实消耗（官方账单）"
        style={{ marginBottom: 16 }}
        extra={
          <span style={{ fontSize: 12, opacity: 0.6 }}>
            合计 {(billing?.total ?? 0).toFixed(1)} 积分 · 近 {billing?.days ?? "—"} 天
          </span>
        }
      >
        <Table<AccountBillingItem>
          rowKey="uid"
          size="small"
          pagination={false}
          loading={loading}
          dataSource={billing?.items ?? []}
          locale={{ emptyText: "暂无账单数据（需账号凭证有效）" }}
          columns={[
            {
              title: "账号",
              render: (_, r) =>
                r.nickname || nameByUid[r.uid] || `…${r.uid.slice(-6)}`,
            },
            {
              title: "真实消耗积分",
              align: "right",
              render: (_, r) => <b>{fmtCredits(r.credits)}</b>,
            },
            {
              title: "计费请求数",
              dataIndex: "requests",
              align: "right",
              width: 120,
            },
          ]}
        />
        <div style={{ fontSize: 12, opacity: 0.5, marginTop: 8 }}>
          取自 WorkBuddy 官方账单接口，为该账号区间内实际扣费（含 App
          端等全渠道），可与官方后台对账。
        </div>
      </Card>

      <Card
        size="small"
        title="请求流水（本工具代理）"
        style={{ marginBottom: 16 }}
      >
        <Table<RequestRow>
          rowKey={(r) => `${r.ts}-${r.uid}-${r.model}`}
          size="small"
          loading={loading}
          dataSource={requests}
          pagination={{
            pageSize: 10,
            size: "small",
            showTotal: (t) => `共 ${t} 条`,
          }}
          locale={{ emptyText: "该时间段暂无请求记录" }}
          columns={[
            {
              title: "时间",
              dataIndex: "ts",
              width: 150,
              render: (v: number) => dayjs(v).format("MM-DD HH:mm:ss"),
            },
            {
              title: "账号",
              render: (_, r) => nameByUid[r.uid] || `…${r.uid.slice(-6)}`,
              ellipsis: true,
            },
            {
              title: "来源",
              dataIndex: "source",
              width: 70,
              render: (v: string) => (
                <Tag color={v === "proxy" ? "blue" : "default"}>
                  {v === "proxy" ? "代理" : "App"}
                </Tag>
              ),
            },
            { title: "模型", dataIndex: "model", ellipsis: true },
            {
              title: "Tokens",
              dataIndex: "totalTokens",
              width: 90,
              align: "right",
            },
            {
              title: "积分(估)",
              dataIndex: "credits",
              width: 90,
              align: "right",
              render: (v: number) => fmtCredits(v),
            },
            {
              title: "耗时",
              dataIndex: "latencyMs",
              width: 80,
              align: "right",
              render: (v: number) => `${v}ms`,
            },
            {
              title: "状态",
              dataIndex: "ok",
              width: 70,
              render: (v: boolean, r) => (
                <Tag color={v ? "green" : "red"}>{r.status}</Tag>
              ),
            },
          ]}
        />
      </Card>

      <Card size="small" title="各账号余额变化">
        {balances.length ? (
          <ReactECharts option={balanceOption} style={{ height: 260 }} notMerge />
        ) : (
          <div style={{ opacity: 0.5, textAlign: "center", padding: 40 }}>
            暂无余额快照，刷新积分后会自动记录。
          </div>
        )}
      </Card>
    </div>
  );
}

function TopTable({ data }: { data: TopItem[] }) {
  return (
    <Table
      rowKey="key"
      size="small"
      pagination={false}
      dataSource={data}
      locale={{ emptyText: "暂无数据" }}
      columns={[
        { title: "名称", dataIndex: "label", ellipsis: true },
        { title: "Tokens", dataIndex: "tokens", width: 100 },
        {
          title: "积分",
          dataIndex: "credits",
          width: 80,
          render: (v: number) => fmtCredits(v),
        },
        { title: "次数", dataIndex: "requests", width: 70 },
      ]}
    />
  );
}
