import { useEffect, useState } from "react";
import {
  Alert,
  Divider,
  Drawer,
  Empty,
  Segmented,
  Spin,
  Table,
  Tag,
  Tooltip,
} from "antd";
import type { ColumnsType } from "antd/es/table";
import { api, errText } from "../api";
import type { CreditLedger, CreditReward, CreditUsageRow } from "../types";

function fmtNum(n: number): string {
  return Math.round(n).toLocaleString("zh-CN");
}

function fmtExpire(ms?: number | null): string {
  if (!ms) return "长期有效";
  const d = new Date(ms);
  if (isNaN(d.getTime())) return "长期有效";
  return d.toLocaleString("zh-CN", { hour12: false });
}

function statusColor(s: string): string {
  if (s === "使用中") return "green";
  if (s === "已过期") return "default";
  if (s === "已用完") return "default";
  return "blue";
}

const usageColumns: ColumnsType<CreditUsageRow> = [
  {
    title: "时间",
    dataIndex: "time",
    width: 180,
    render: (t: string) => (
      <span style={{ fontSize: 12 }}>{t.replace("T", " ").slice(0, 19)}</span>
    ),
  },
  {
    title: "请求",
    dataIndex: "requestId",
    ellipsis: true,
    render: (v: string) => (
      <Tooltip title={v}>
        <span style={{ fontSize: 12, fontFamily: "monospace" }}>
          {v ? v.slice(0, 12) : "—"}
        </span>
      </Tooltip>
    ),
  },
  {
    title: "积分消耗",
    dataIndex: "credits",
    width: 100,
    align: "right",
    sorter: (a, b) => a.credits - b.credits,
    render: (v: number) => <b>{v.toLocaleString("zh-CN")}</b>,
  },
  { title: "模型", dataIndex: "model", width: 160, ellipsis: true },
  { title: "使用端", dataIndex: "client", width: 120, ellipsis: true },
];

export default function CreditLedgerDrawer({
  open,
  onClose,
  uid,
  nickname,
}: {
  open: boolean;
  onClose: () => void;
  uid: string;
  nickname: string;
}) {
  const [days, setDays] = useState(7);
  const [loading, setLoading] = useState(false);
  const [data, setData] = useState<CreditLedger | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    if (!open || !uid) return;
    let cancelled = false;
    setLoading(true);
    setError("");
    api
      .creditLedger(uid, days)
      .then((res) => {
        if (!cancelled) setData(res);
      })
      .catch((e) => {
        if (!cancelled) setError(errText(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, uid, days]);

  const rewards: CreditReward[] = data?.rewards ?? [];
  const usage: CreditUsageRow[] = data?.usage ?? [];

  return (
    <Drawer
      open={open}
      onClose={onClose}
      width={720}
      title={`积分清单 · ${nickname || "账号"}`}
      destroyOnClose
    >
      {error && (
        <Alert
          type="error"
          showIcon
          style={{ marginBottom: 16 }}
          message="加载失败"
          description={error}
        />
      )}
      <Spin spinning={loading}>
        {/* 平台奖励积分 */}
        <div style={{ display: "flex", alignItems: "center", marginBottom: 12 }}>
          <span style={{ fontSize: 16, fontWeight: 600 }}>平台奖励积分</span>
          <Tooltip title="官方活动发放、签到、做任务、派猫猫旅行等获得的积分批次，各有独立到期时间">
            <span style={{ marginLeft: 6, opacity: 0.5 }}>ⓘ</span>
          </Tooltip>
        </div>

        {rewards.length === 0 ? (
          <Empty
            description={loading ? "加载中…" : "暂无奖励/购买积分记录"}
            image={Empty.PRESENTED_IMAGE_SIMPLE}
          />
        ) : (
          <>
            <div
              style={{
                maxHeight: 320,
                overflowY: "auto",
                border: "1px solid rgba(255,255,255,0.08)",
                borderRadius: 8,
                padding: "4px 0",
              }}
            >
              {rewards.map((r, idx) => (
                <div
                  key={`${r.name}-${idx}-${r.expireAt ?? ""}`}
                  style={{
                    padding: "10px 14px",
                    borderBottom:
                      idx === rewards.length - 1
                        ? "none"
                        : "1px solid rgba(255,255,255,0.06)",
                  }}
                >
                  <div
                    style={{
                      display: "flex",
                      justifyContent: "space-between",
                      alignItems: "center",
                    }}
                  >
                    <span>
                      <span style={{ fontWeight: 600 }}>
                        {fmtNum(r.used)}/{fmtNum(r.total)}积分
                      </span>
                      <span style={{ opacity: 0.55, fontSize: 12 }}>
                        {" "}
                        （已用/总量）
                      </span>
                      <Tag color={statusColor(r.status)} style={{ marginLeft: 8 }}>
                        {r.status}
                      </Tag>
                    </span>
                    <span style={{ opacity: 0.7, fontSize: 12 }}>
                      到期时间：{fmtExpire(r.expireAt)}
                    </span>
                  </div>
                  <div style={{ marginTop: 4, fontSize: 12, opacity: 0.6 }}>
                    {r.name}
                    {r.reason ? ` · 下发原因：${r.reason}` : ""}
                  </div>
                </div>
              ))}
            </div>
            <div style={{ marginTop: 8, fontSize: 13, opacity: 0.8 }}>
              可用{data?.rewardCount ?? 0}个 · 剩余
              {fmtNum(data?.rewardRemaining ?? 0)}积分
            </div>
          </>
        )}

        <Divider style={{ margin: "20px 0" }} />

        {/* 积分消耗明细 */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            marginBottom: 12,
          }}
        >
          <span style={{ fontSize: 16, fontWeight: 600 }}>积分消耗明细</span>
          <Segmented
            value={days}
            onChange={(v) => setDays(v as number)}
            options={[
              { label: "今天", value: 1 },
              { label: "7天", value: 7 },
              { label: "30天", value: 30 },
            ]}
          />
        </div>

        <Table<CreditUsageRow>
          size="small"
          rowKey={(r) => `${r.ts}-${r.requestId}`}
          columns={usageColumns}
          dataSource={usage}
          loading={loading}
          pagination={{ pageSize: 15, size: "small", showTotal: (t) => `共 ${t} 条` }}
          locale={{ emptyText: <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="该时间段无消耗记录" /> }}
          summary={() =>
            usage.length > 0 ? (
              <Table.Summary.Row>
                <Table.Summary.Cell index={0} colSpan={2}>
                  合计消耗
                </Table.Summary.Cell>
                <Table.Summary.Cell index={2} align="right">
                  <b>{fmtNum(data?.usageTotal ?? 0)}</b>
                </Table.Summary.Cell>
                <Table.Summary.Cell index={3} colSpan={2} />
              </Table.Summary.Row>
            ) : null
          }
        />
      </Spin>
    </Drawer>
  );
}
