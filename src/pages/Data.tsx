import { useEffect, useMemo, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Col,
  Empty,
  Input,
  List,
  message,
  Modal,
  Popconfirm,
  Row,
  Select,
  Space,
  Table,
  Tag,
  Tooltip,
  Tree,
} from "antd";
import {
  DownloadOutlined,
  DeleteOutlined,
  EyeOutlined,
  SaveOutlined,
  SwapOutlined,
  MessageOutlined,
  BulbOutlined,
  SettingOutlined,
  ThunderboltOutlined,
  FolderOutlined,
} from "@ant-design/icons";
import { api, errText } from "../api";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useStore } from "../store";
import type { DataItem } from "../types";

const CATS = [
  { key: "sessions", title: "会话", icon: <MessageOutlined /> },
  { key: "memory", title: "记忆", icon: <BulbOutlined /> },
  { key: "agent-config", title: "Agent 设定", icon: <SettingOutlined /> },
  { key: "skills", title: "技能", icon: <ThunderboltOutlined /> },
  { key: "artifacts", title: "产出文件", icon: <FolderOutlined /> },
];

function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}
function fmtDate(ms: number): string {
  if (!ms) return "—";
  return new Date(ms).toLocaleString("zh-CN", { hour12: false });
}

export default function DataPage() {
  const info = useStore((s) => s.info);
  const accounts = useStore((s) => s.accounts);
  const [uids, setUids] = useState<string[]>([]);
  const [uid, setUid] = useState<string>("");
  const [cat, setCat] = useState("sessions");
  const [items, setItems] = useState<DataItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<DataItem[]>([]);

  const [preview, setPreview] = useState<{ title: string; body: string } | null>(
    null,
  );
  const [backupModal, setBackupModal] = useState(false);
  const [backupLabel, setBackupLabel] = useState("backup");
  const [backupDest, setBackupDest] = useState("");
  const [migrateModal, setMigrateModal] = useState(false);
  const [migrateTo, setMigrateTo] = useState("");
  const [migrateCats, setMigrateCats] = useState<string[]>(["memory", "skills"]);

  const defaultExportDir = `${info?.paths.workmamiRoot || "."}/exports`;

  // uid → 昵称映射：已知账号显示"昵称（uid 前 8 位）"，未知账号退回裸 uid
  const uidLabel = (u: string) => {
    const acc = accounts.find((a) => a.uid === u);
    return acc?.nickname ? `${acc.nickname}（${u.slice(0, 8)}…）` : u;
  };

  useEffect(() => {
    api
      .dataUids()
      .then((u) => setUids(u))
      .catch((e) => message.error(errText(e)));
  }, []);

  // 排序：已知账号（按 accounts 顺序）优先，其后是仅本机发现的其它 uid。
  const orderedUids = useMemo(() => {
    const known = accounts.map((a) => a.uid).filter((u) => uids.includes(u));
    const rest = uids.filter((u) => !known.includes(u));
    return [...known, ...rest];
  }, [accounts, uids]);

  // 首次拿到列表时默认选中第一个已知账号（数据最全的那个），避免落在空账号上被误判为仍坏。
  useEffect(() => {
    if (!uid && orderedUids.length) setUid(orderedUids[0]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [orderedUids]);

  const load = async () => {
    setLoading(true);
    try {
      const list = await api.dataList(cat, uid || undefined);
      setItems(list);
      setSelected([]);
    } catch (e) {
      message.error(errText(e));
      setItems([]);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cat, uid]);

  const view = async (item: DataItem) => {
    try {
      const body = await api.dataRead(item);
      setPreview({ title: item.name, body });
    } catch (e) {
      message.error(errText(e));
    }
  };

  const del = async (list: DataItem[]) => {
    try {
      const n = await api.dataDelete(list);
      message.success(`已移入回收站 ${n} 项`);
      await load();
    } catch (e) {
      message.error(errText(e));
    }
  };

  // 导出：直接弹原生目录选择器，选完即导出
  const doExport = async (item: DataItem) => {
    const dir = await openDialog({
      directory: true,
      defaultPath: defaultExportDir,
      title: "选择导出位置",
    });
    if (!dir) return; // 用户取消
    try {
      const p = await api.dataExport(item, String(dir));
      message.success(`已导出到 ${p}`);
    } catch (e) {
      message.error(errText(e));
    }
  };

  const pickBackupDir = async () => {
    const dir = await openDialog({
      directory: true,
      defaultPath: backupDest || info?.paths.workmamiRoot,
      title: "选择备份保存位置",
    });
    if (dir) setBackupDest(String(dir));
  };

  const doBackup = async () => {
    try {
      const p = await api.dataBackup(selected.length ? selected : items, backupLabel, backupDest || undefined);
      message.success(`备份完成：${p}`);
      setBackupModal(false);
      setBackupDest("");
    } catch (e) {
      message.error(errText(e));
    }
  };

  const runMigrate = async () => {
    const n = await api.dataMigrate(uid, migrateTo, migrateCats);
    message.success(`已迁移 ${n} 个条目`);
    await load();
  };

  const confirmAsync = (opts: object) =>
    new Promise<boolean>((resolve) => {
      Modal.confirm({
        ...opts,
        onOk: () => resolve(true),
        onCancel: () => resolve(false),
      });
    });

  const doMigrate = async () => {
    if (!uid || !migrateTo) {
      message.warning("请选择目标账号");
      return Promise.reject();
    }
    if (migrateCats.includes("sessions")) {
      const ok = await confirmAsync({
        title: "确认迁移会话？",
        content:
          "这会直接改写 WorkBuddy 的 workbuddy.db（把会话归属改到目标账号）。系统会先自动整库备份。请确认已完全退出 WorkBuddy。",
        okText: "我已退出 WorkBuddy，继续",
        cancelText: "取消",
      });
      if (!ok) return Promise.reject();
    }
    try {
      await runMigrate();
      setMigrateModal(false);
    } catch (e) {
      message.error(errText(e));
      return Promise.reject();
    }
  };

  const columns = useMemo(
    () => [
      {
        title: "名称",
        dataIndex: "name",
        ellipsis: true,
        render: (t: string, r: DataItem) => (
          <Space>
            <a onClick={() => view(r)}>{t}</a>
            {r.meta && "model" in (r.meta as object) && (
              <Tag>{String((r.meta as Record<string, unknown>).model)}</Tag>
            )}
          </Space>
        ),
      },
      {
        title: "大小",
        dataIndex: "size",
        width: 100,
        render: (n: number) => (n ? fmtSize(n) : "—"),
      },
      {
        title: "修改时间",
        dataIndex: "modified",
        width: 180,
        render: fmtDate,
      },
      {
        title: "操作",
        width: 180,
        render: (_: unknown, r: DataItem) => (
          <Space>
            <Tooltip title="查看">
              <Button size="small" icon={<EyeOutlined />} onClick={() => view(r)} />
            </Tooltip>
            <Tooltip title="导出">
              <Button
                size="small"
                icon={<DownloadOutlined />}
                onClick={() => doExport(r)}
              />
            </Tooltip>
            <Popconfirm title="移入回收站？" onConfirm={() => del([r])} okText="删除">
              <Button size="small" danger icon={<DeleteOutlined />} />
            </Popconfirm>
          </Space>
        ),
      },
    ],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [uid, defaultExportDir],
  );

  return (
    <div className="wm-page">
      <Row gutter={16}>
        <Col xs={24} md={5}>
          <Card title="类别" size="small">
            <Tree
              selectedKeys={[cat]}
              onSelect={(keys) => keys[0] && setCat(String(keys[0]))}
              treeData={CATS.map((c) => ({
                key: c.key,
                title: (
                  <Space size={8}>
                    {c.icon}
                    <span>{c.title}</span>
                  </Space>
                ),
              }))}
            />
          </Card>
        </Col>
        <Col xs={24} md={19}>
          <Card
            size="small"
            title={
              <Space>
                <span>账号数据源</span>
                <Select
                  size="small"
                  style={{ width: 220 }}
                  placeholder="选择账号"
                  allowClear
                  value={uid || undefined}
                  onChange={(v) => setUid(v || "")}
                  options={[
                    { value: "", label: "全部 / 全局" },
                    ...orderedUids.map((u) => ({ value: u, label: uidLabel(u) })),
                  ]}
                />
              </Space>
            }
            extra={
              <Space>
                <Button
                  icon={<SaveOutlined />}
                  disabled={!items.length}
                  onClick={() => setBackupModal(true)}
                >
                  打包备份
                </Button>
                <Button
                  icon={<SwapOutlined />}
                  disabled={cat === "artifacts"}
                  onClick={() => setMigrateModal(true)}
                  title="产出文件不属于任何账号（全局共享），无需迁移"
                >
                  跨账号迁移
                </Button>
                <Popconfirm
                  title={`确认删除选中的 ${selected.length} 项？`}
                  disabled={!selected.length}
                  onConfirm={() => del(selected)}
                  okText="删除"
                >
                  <Button danger icon={<DeleteOutlined />} disabled={!selected.length}>
                    删除选中
                  </Button>
                </Popconfirm>
              </Space>
            }
          >
            <Table
              rowKey={(r) => r.id + r.path}
              size="small"
              loading={loading}
              dataSource={items}
              columns={columns}
              rowSelection={{
                selectedRowKeys: selected.map((s) => s.id + s.path),
                onChange: (_, rows) => setSelected(rows),
              }}
              pagination={{
                pageSize: 15,
                showSizeChanger: true,
                pageSizeOptions: [15, 50, 100, 200],
                showTotal: (t) => `共 ${t} 条`,
              }}
              locale={{ emptyText: <Empty description="该类别暂无数据" /> }}
            />
          </Card>
        </Col>
      </Row>

      {/* 预览 */}
      <Modal
        open={!!preview}
        title={preview?.title}
        width={860}
        footer={<Button onClick={() => setPreview(null)}>关闭</Button>}
        onCancel={() => setPreview(null)}
      >
        <div className="wm-preview">{preview?.body}</div>
      </Modal>

      {/* 备份 */}
      <Modal
        open={backupModal}
        title="打包备份"
        onCancel={() => setBackupModal(false)}
        onOk={doBackup}
        okText="开始备份"
      >
        <p>
          将{selected.length ? `选中的 ${selected.length}` : `当前类别全部 ${items.length}`}
          条条目打包为 zip（含 manifest）。
        </p>
        <Input
          addonBefore="标签"
          value={backupLabel}
          onChange={(e) => setBackupLabel(e.target.value)}
        />
        <div style={{ marginTop: 12 }}>
          <span style={{ display: "block", marginBottom: 4 }}>保存位置（不选则存入 WorkMami 备份目录）：</span>
          <Space.Compact style={{ width: "100%" }}>
            <Input
              readOnly
              placeholder="默认：WorkMami/Backups"
              value={backupDest}
            />
            <Button onClick={pickBackupDir}>浏览…</Button>
          </Space.Compact>
        </div>
      </Modal>

      {/* 迁移 */}
      <Modal
        open={migrateModal}
        title="跨账号迁移"
        onCancel={() => setMigrateModal(false)}
        onOk={doMigrate}
        okText="执行迁移"
      >
        <List
          dataSource={[]}
          header={
            <Space direction="vertical" style={{ width: "100%" }}>
              <span>
                从 <Tag>{uid ? uidLabel(uid) : "全局"}</Tag> 迁移到：
              </span>
              <Select
                style={{ width: "100%" }}
                placeholder="目标账号"
                value={migrateTo || undefined}
                onChange={setMigrateTo}
                options={uids.filter((u) => u !== uid).map((u) => ({ value: u, label: uidLabel(u) }))}
              />
              <span>选择迁移类别：</span>
              <Select
                mode="multiple"
                style={{ width: "100%" }}
                value={migrateCats}
                onChange={setMigrateCats}
                options={[
                  { value: "sessions", label: "会话" },
                  { value: "memory", label: "记忆" },
                  { value: "agent-config", label: "Agent 设定" },
                  { value: "skills", label: "技能" },
                ]}
              />
            </Space>
          }
        />
        {migrateCats.includes("sessions") && (
          <Alert
            type="warning"
            showIcon
            style={{ marginTop: 12 }}
            message="会话迁移会改写 WorkBuddy 的 workbuddy.db（把会话归属改到目标账号）"
            description="系统会先自动整库备份以便回滚。强烈建议先完全退出 WorkBuddy 再执行，以免数据库被占用或官方客户端缓存覆盖。"
          />
        )}
        <p style={{ marginTop: 12 }}>
          冲突文件将自动重命名保留双方。记忆/Agent 设定为复制迁移（保留源），会话为改归属迁移（源账号不再展示）。
        </p>
      </Modal>
    </div>
  );
}
