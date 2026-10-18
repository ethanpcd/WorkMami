import { useEffect, useMemo, useState } from "react";
import {
  Alert,
  Badge,
  Button,
  Card,
  Col,
  Empty,
  Input,
  message,
  Modal,
  Popconfirm,
  Progress,
  Row,
  Space,
  Switch,
  Tag,
  Tooltip,
} from "antd";
import {
  DeleteOutlined,
  EditOutlined,
  ProfileOutlined,
  ReloadOutlined,
  UserAddOutlined,
} from "@ant-design/icons";
import { api, errText } from "../api";
import { useStore } from "../store";
import type { Account } from "../types";
import CreditLedgerDrawer from "./CreditLedgerDrawer";

const sourceLabel: Record<string, string> = {
  oauth: "网页授权",
  scan: "扫描",
  manual: "手动",
};

function fmtTime(s?: string | null): string {
  if (!s) return "—";
  const d = new Date(s);
  if (isNaN(d.getTime())) return "—";
  return d.toLocaleString("zh-CN", { hour12: false });
}

function fmtNum(n: number): string {
  return Math.round(n).toLocaleString("zh-CN");
}

// 到期时间展示：日期 + 相对天数；无到期视为长期有效。
function fmtExpire(ms?: number | null): { text: string; days: number | null } {
  if (!ms) return { text: "长期有效", days: null };
  const d = new Date(ms);
  if (isNaN(d.getTime())) return { text: "长期有效", days: null };
  const days = Math.ceil((ms - Date.now()) / 86400000);
  const date = d.toLocaleDateString("zh-CN");
  if (days < 0) return { text: `${date}（已过期）`, days };
  return { text: `${date}（剩 ${days} 天）`, days };
}

export default function Accounts() {
  const accounts = useStore((s) => s.accounts);
  const reloadAccounts = useStore((s) => s.reloadAccounts);

  const [refreshing, setRefreshing] = useState(false);

  // 积分清单抽屉
  const [ledgerUid, setLedgerUid] = useState("");
  const [ledgerName, setLedgerName] = useState("");

  // OAuth 弹窗
  const [oauthOpen, setOauthOpen] = useState(false);
  const [oauthSession, setOauthSession] = useState("");
  const [oauthStatus, setOauthStatus] = useState<
    "idle" | "pending" | "authorized" | "expired"
  >("idle");

  // 标签编辑
  const [tagOpen, setTagOpen] = useState(false);
  const [tagUid, setTagUid] = useState("");
  const [tagValue, setTagValue] = useState<string>("");

  useEffect(() => {
    let timer: ReturnType<typeof setInterval> | undefined;
    // 只要有待抓取的会话就持续轮询，与弹窗是否显示无关（点「后台等待」也不会中断）。
    if (oauthSession && oauthStatus === "pending") {
      timer = setInterval(async () => {
        try {
          const res = await api.oauthPoll(oauthSession);
          if (res.status === "authorized") {
            setOauthStatus("authorized");
            message.success("登录成功，账号已入库");
            await reloadAccounts();
            // 自动关闭弹窗并复位，登录窗口由后端负责关闭
            setOauthOpen(false);
            setOauthSession("");
            setOauthStatus("idle");
            clearInterval(timer);
          } else if (res.status === "expired") {
            setOauthStatus("expired");
            setOauthSession("");
            clearInterval(timer);
          }
        } catch (e) {
          setOauthStatus("expired");
          setOauthSession("");
          clearInterval(timer);
          message.error(errText(e));
        }
      }, 2000);
    }
    return () => timer && clearInterval(timer);
  }, [oauthSession, oauthStatus, reloadAccounts]);

  const startOAuth = async () => {
    setOauthOpen(true);
    setOauthStatus("pending");
    setOauthSession("");
    try {
      const res = await api.oauthBegin();
      setOauthSession(res.sessionId);
    } catch (e) {
      setOauthStatus("expired");
      message.error(errText(e));
    }
  };

  const refreshAll = async () => {
    setRefreshing(true);
    try {
      const rs = await api.refreshQuota();
      const ok = rs.filter((r) => r.ok).length;
      message.success(`刷新完成：成功 ${ok}/${rs.length}`);
      await reloadAccounts();
    } catch (e) {
      message.error(errText(e));
    } finally {
      setRefreshing(false);
    }
  };

  const refreshOne = async (uid: string) => {
    try {
      const rs = await api.refreshQuota([uid]);
      if (rs[0]?.ok) message.success("积分已刷新");
      else message.warning(rs[0]?.error || "刷新失败");
      await reloadAccounts();
    } catch (e) {
      message.error(errText(e));
    }
  };

  const saveTags = async () => {
    const tags = tagValue
      .split(/[,，\s]+/)
      .map((t) => t.trim())
      .filter(Boolean);
    try {
      await api.setAccountTags(tagUid, tags);
      message.success("标签已更新");
      setTagOpen(false);
      await reloadAccounts();
    } catch (e) {
      message.error(errText(e));
    }
  };

  const totalRemaining = useMemo(
    () => accounts.reduce((s, a) => s + (a.quota?.remaining || 0), 0),
    [accounts],
  );

  return (
    <div className="wm-page">
      <Alert
        type="info"
        showIcon
        style={{ marginBottom: 16 }}
        message="账号通过网页授权添加"
        description="点击「网页授权添加账号」会弹出一个 WorkBuddy 登录窗口，登录后自动抓取凭证入库。授权后即可刷新积分、签到、派猫猫旅行并接入本地反代。"
      />

      <div className="wm-toolbar">
        <Button type="primary" icon={<UserAddOutlined />} onClick={startOAuth}>
          网页授权添加账号
        </Button>
        <Button icon={<ReloadOutlined />} loading={refreshing} onClick={refreshAll}>
          刷新全部积分
        </Button>
        <span style={{ marginLeft: "auto", opacity: 0.7 }}>
          共 {accounts.length} 个账号 · 剩余积分合计 {Math.round(totalRemaining)}
        </span>
      </div>

      {accounts.length === 0 ? (
        <Empty description="暂无账号，点击「网页授权添加账号」开始" />
      ) : (
        <Row gutter={[16, 16]}>
          {accounts.map((a) => (
            <Col key={a.uid} xs={24} md={12} xl={8}>
              <AccountCard
                a={a}
                onRefresh={() => refreshOne(a.uid)}
                onLedger={() => {
                  setLedgerUid(a.uid);
                  setLedgerName(a.nickname);
                }}
                onToggle={async (v) => {
                  await api.setAccountEnabled(a.uid, v);
                  await reloadAccounts();
                }}
                onDelete={async () => {
                  await api.deleteAccount(a.uid);
                  message.success("已删除");
                  await reloadAccounts();
                }}
                onTags={() => {
                  setTagUid(a.uid);
                  setTagValue(a.tags.join(", "));
                  setTagOpen(true);
                }}
              />
            </Col>
          ))}
        </Row>
      )}

      {/* OAuth 授权弹窗 */}
      <Modal
        open={oauthOpen}
        title="网页授权添加账号"
        onCancel={() => setOauthOpen(false)}
        footer={
          oauthStatus === "authorized" || oauthStatus === "expired"
            ? [
                <Button key="c" type="primary" onClick={() => setOauthOpen(false)}>
                  关闭
                </Button>,
              ]
            : [
                <Button key="c" onClick={() => setOauthOpen(false)}>
                  后台等待
                </Button>,
              ]
        }
      >
        {oauthStatus === "pending" && (
          <Alert
            type="info"
            showIcon
            message="请在弹出的登录窗口中完成登录"
            description="已为你打开 WorkBuddy 登录窗口，请使用微信扫码或手机号登录。登录成功后 WorkMami 会自动抓取凭证并入库，无需其他操作。"
          />
        )}
        {oauthStatus === "authorized" && (
          <Alert type="success" showIcon message="登录成功，账号已入库" />
        )}
        {oauthStatus === "expired" && (
          <Alert
            type="error"
            showIcon
            message="未抓取到凭证"
            description="可能你关闭了登录窗口、登录超时，或该页面未把 token 存到可读位置。可重试；若反复失败请反馈。"
          />
        )}
      </Modal>

      {/* 标签编辑 */}
      <Modal
        open={tagOpen}
        title="编辑标签"
        onCancel={() => setTagOpen(false)}
        onOk={saveTags}
      >
        <Input
          value={tagValue}
          onChange={(e) => setTagValue(e.target.value)}
          placeholder="多个标签用逗号分隔，例如：主力,测试"
        />
      </Modal>

      {/* 积分清单抽屉 */}
      <CreditLedgerDrawer
        open={!!ledgerUid}
        onClose={() => setLedgerUid("")}
        uid={ledgerUid}
        nickname={ledgerName}
      />
    </div>
  );
}

function AccountCard({
  a,
  onRefresh,
  onLedger,
  onToggle,
  onDelete,
  onTags,
}: {
  a: Account;
  onRefresh: () => void;
  onLedger: () => void;
  onToggle: (v: boolean) => void;
  onDelete: () => void;
  onTags: () => void;
}) {
  const q = a.quota;
  const [open, setOpen] = useState(false);
  const pct = q && q.total > 0 ? Math.round((q.remaining / q.total) * 100) : 0;
  // 卡片概览只列「还有剩余」的批次（按到期升序）：已用完的批次没有到期压力，
  // 列出「0 · 剩 X 天」会让用户困惑，故折叠成一句汇总，完整历史看积分清单抽屉。
  const activeResources = (q?.resources || [])
    .filter((r) => r.remaining > 0)
    .sort((a, b) => (a.expire_at ?? Infinity) - (b.expire_at ?? Infinity));
  const usedUpCount = (q?.resources?.length || 0) - activeResources.length;
  const uidTail = a.uid.length > 4 ? "…" + a.uid.slice(-4) : a.uid;

  return (
    <Card
      size="small"
      title={
        <Space>
          <Badge status={a.enabled ? "processing" : "default"} />
          <span>{a.nickname || "未命名"}</span>
          <Tooltip title={`UID: ${a.uid}`}>
            <Tag className="wm-mono">{uidTail}</Tag>
          </Tooltip>
        </Space>
      }
      extra={<Switch size="small" checked={a.enabled} onChange={onToggle} />}
      actions={[
        <Tooltip title="刷新积分" key="r">
          <ReloadOutlined onClick={onRefresh} />
        </Tooltip>,
        <Tooltip title="积分清单" key="l">
          <ProfileOutlined onClick={onLedger} />
        </Tooltip>,
        <Tooltip title="编辑标签" key="t">
          <EditOutlined onClick={onTags} />
        </Tooltip>,
        <Popconfirm
          key="d"
          title="确定删除该账号？"
          description="仅移除 WorkMami 记录，不影响官方登录文件。"
          onConfirm={onDelete}
          okText="删除"
          cancelText="取消"
        >
          <DeleteOutlined />
        </Popconfirm>,
      ]}
    >
      <Space direction="vertical" style={{ width: "100%" }} size={8}>
        <Space size={4} wrap>
          <Tag>{sourceLabel[a.source] || a.source}</Tag>
          <Tag color={a.account_type === "enterprise" ? "gold" : "blue"}>
            {a.account_type === "enterprise" ? "企业" : "个人"}
          </Tag>
          {a.has_local_token ? (
            <Tag color="green">凭证就绪</Tag>
          ) : (
            <Tag color="red">需重新授权</Tag>
          )}
          {a.today_checked_in && <Tag color="purple">今日已签</Tag>}
          {a.streak_days > 0 && <Tag>连续 {a.streak_days} 天</Tag>}
        </Space>

        {a.tags.length > 0 && (
          <Space size={4} wrap>
            {a.tags.map((t) => (
              <Tag key={t} color="cyan">
                {t}
              </Tag>
            ))}
          </Space>
        )}

        {q ? (
          <div>
            <div
              style={{
                display: "flex",
                alignItems: "baseline",
                justifyContent: "space-between",
              }}
            >
              <span>
                剩余{" "}
                <span style={{ fontSize: 22, fontWeight: 700 }}>
                  {fmtNum(q.remaining)}
                </span>{" "}
                <span style={{ opacity: 0.5, fontSize: 12 }}>/ {fmtNum(q.total)}</span>
              </span>
              {q.expiring_soon && <Tag color="orange">有积分即将到期</Tag>}
            </div>
            <Progress
              percent={pct}
              status={pct < 15 ? "exception" : "normal"}
              showInfo={false}
            />
            <div style={{ fontSize: 12, opacity: 0.6 }}>
              已用 {fmtNum(q.used)} · 更新于 {fmtTime(q.updated_at)}
            </div>
            {(activeResources.length > 0 || usedUpCount > 0) && (
              <div style={{ marginTop: 4 }}>
                {activeResources.length > 0 && (
                  <a
                    style={{ fontSize: 12 }}
                    onClick={() => setOpen((v) => !v)}
                  >
                    {open ? "收起" : "展开"}可用积分明细（{activeResources.length}）
                  </a>
                )}
                {open && activeResources.length > 0 && (
                  <div
                    style={{
                      marginTop: 6,
                      border: "1px solid rgba(255,255,255,0.08)",
                      borderRadius: 6,
                      overflow: "hidden",
                    }}
                  >
                    <div style={{ maxHeight: 220, overflowY: "auto" }}>
                      {activeResources.map((r, i) => {
                        const e = fmtExpire(r.expire_at);
                        const color = r.expired
                          ? "#ff4d4f"
                          : r.expiring_soon
                            ? "#faad14"
                            : undefined;
                        return (
                          <div
                            key={r.package_code + r.name + (r.expire_at ?? "") + i}
                            style={{
                              display: "flex",
                              justifyContent: "space-between",
                              alignItems: "center",
                              padding: "4px 8px",
                              fontSize: 12,
                              gap: 8,
                            }}
                          >
                            <span style={{ opacity: 0.85 }}>{r.name}</span>
                            <span style={{ display: "flex", gap: 10 }}>
                              <b style={{ color }}>{fmtNum(r.remaining)}</b>
                              <span style={{ opacity: 0.55, color }}>{e.text}</span>
                            </span>
                          </div>
                        );
                      })}
                    </div>
                  </div>
                )}
                {usedUpCount > 0 && (
                  <div style={{ fontSize: 11, opacity: 0.45, marginTop: 4 }}>
                    另有 {usedUpCount} 笔已用完 · 完整记录见「积分清单」
                  </div>
                )}
              </div>
            )}
          </div>
        ) : (
          <div style={{ opacity: 0.5 }}>积分未同步，点击刷新</div>
        )}

        <div style={{ fontSize: 12, opacity: 0.6 }}>
          最近使用：{fmtTime(a.last_used_at)}
        </div>
      </Space>
    </Card>
  );
}
