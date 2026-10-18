import { useEffect, useMemo, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Col,
  Divider,
  Form,
  Input,
  InputNumber,
  message,
  Row,
  Select,
  Space,
  Switch,
  Tag,
  Tooltip,
  Typography,
} from "antd";
import { CopyOutlined, ReloadOutlined } from "@ant-design/icons";
import { api, errText } from "../api";
import { useStore } from "../store";
import type { ModelInfo, ProxySettings as PS } from "../types";

const { Paragraph, Text } = Typography;

/// 生成一串 SK- 开头的随机访问密钥（无需用户自设）。
function genApiKey(): string {
  const arr = new Uint8Array(16);
  crypto.getRandomValues(arr);
  const hex = Array.from(arr, (b) => b.toString(16).padStart(2, "0")).join("");
  return `SK-${hex}`;
}

const copyText = (s: string, tip: string) => {
  navigator.clipboard.writeText(s);
  message.success(tip);
};

export default function Proxy() {
  const settings = useStore((s) => s.settings);
  const proxy = useStore((s) => s.proxy);
  const saveSettings = useStore((s) => s.saveSettings);
  const reloadProxy = useStore((s) => s.reloadProxy);

  const [form] = Form.useForm();
  const [saving, setSaving] = useState(false);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [modelsError, setModelsError] = useState("");

  useEffect(() => {
    const p = settings.proxy;
    form.setFieldsValue(p);
    // 密钥为空时自动生成一串 SK- 开头的随机 key，用户无需自己设置。
    if (!p.apiKey) form.setFieldValue("apiKey", genApiKey());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings.proxy]);

  // 进入页面即拉取一次 WorkBuddy 内置模型列表（实时，服务端上新即反映）。
  const loadModels = async (silent = false) => {
    setModelsLoading(true);
    setModelsError("");
    try {
      const list = await api.listModels();
      setModels(list);
      if (list.length === 0) {
        setModelsError("未获取到模型，可能当前账号无权限");
      } else if (!silent) {
        message.success(`已获取 ${list.length} 个 WorkBuddy 模型`);
      }
    } catch (e) {
      setModelsError(errText(e) || "获取模型列表失败");
    } finally {
      setModelsLoading(false);
    }
  };
  useEffect(() => {
    loadModels(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 选中模型后，以其官方参数为准自动回填上下文窗口 / 思考强度（仍可手动微调）。
  const onModelChange = (id: string) => {
    const m = models.find((x) => x.id === id);
    if (!m) return;
    if (m.contextWindow > 0) form.setFieldValue("contextWindow", m.contextWindow);
    if (m.defaultEffort) form.setFieldValue("reasoningEffort", m.defaultEffort);
  };

  const onSave = async () => {
    try {
      const v = await form.validateFields();
      const next: PS = { ...settings.proxy, ...v };
      setSaving(true);
      await saveSettings({ ...settings, proxy: next });
      message.success("代理设置已保存");
      await reloadProxy();
    } catch (e) {
      if (errText(e)) message.error(errText(e));
    } finally {
      setSaving(false);
    }
  };

  const enabled = Form.useWatch("enabled", form);
  const apiKey = Form.useWatch("apiKey", form);
  const defaultModel = Form.useWatch("defaultModel", form);

  // 模型下拉：以官方列表为准；若已保存的默认模型不在列表中（自定义/未刷新），补进去以免丢失显示。
  const modelOptions = useMemo(() => {
    const seen = new Set<string>();
    const opts = models.map((m) => {
      seen.add(m.id);
      return { value: m.id, label: m.name === m.id ? m.id : `${m.name} · ${m.id}` };
    });
    if (defaultModel && !seen.has(defaultModel)) {
      // 未命中已拉取列表时（如列表为空），默认模型统一以品牌名展示，底层值仍是真实上游 id。
      opts.unshift({ value: defaultModel, label: "WorkMami Model" });
    }
    return opts;
  }, [models, defaultModel]);
  const selectedModel = models.find((m) => m.id === defaultModel);

  const baseUrl = proxy?.baseUrl || `http://${settings.proxy.host}:${settings.proxy.port}/v1`;

  return (
    <div className="wm-page">
      <Row gutter={16}>
        <Col xs={24} lg={14}>
          <Card title="本地反向代理" size="small">
            <Form form={form} layout="vertical" initialValues={settings.proxy}>
              <Space size="large" wrap>
                <Form.Item
                  name="enabled"
                  label="启用反代"
                  valuePropName="checked"
                >
                  <Switch />
                </Form.Item>
                <Form.Item name="host" label="监听地址">
                  <Input style={{ width: 140 }} />
                </Form.Item>
                <Form.Item name="port" label="端口">
                  <InputNumber style={{ width: 100 }} min={1} max={65535} />
                </Form.Item>
              </Space>

              {!enabled && (
                <Alert
                  type="warning"
                  showIcon
                  style={{ marginBottom: 16 }}
                  message="反代未启用，保存后不会启动 HTTP 服务。"
                />
              )}
              {(!apiKey || apiKey === "") && (
                <Alert
                  type="error"
                  showIcon
                  style={{ marginBottom: 16 }}
                  message="API Key 为空：任何本地进程都能调用你的账号积分，建议设置密钥。"
                />
              )}

              <Form.Item
                label="访问密钥 (Bearer Key)"
                tooltip="已自动生成，无需手动设置；客户端需在 Authorization: Bearer <key> 中携带"
              >
                <Space.Compact style={{ width: "100%" }}>
                  <Form.Item name="apiKey" noStyle>
                    <Input.Password
                      className="wm-mono"
                      placeholder="自动生成 SK-… 随机密钥"
                      visibilityToggle
                    />
                  </Form.Item>
                  <Tooltip title="随机生成新密钥">
                    <Button
                      icon={<ReloadOutlined />}
                      onClick={() => {
                        form.setFieldValue("apiKey", genApiKey());
                        message.success("已生成新密钥，记得保存");
                      }}
                    />
                  </Tooltip>
                  <Tooltip title="复制密钥">
                    <Button
                      icon={<CopyOutlined />}
                      onClick={() => copyText(apiKey || "", "已复制 API Key")}
                    />
                  </Tooltip>
                </Space.Compact>
              </Form.Item>

              <Divider orientation="left" plain>
                调度与模型
              </Divider>

              <Row gutter={12}>
                <Col span={12}>
                  <Form.Item name="strategy" label="调度策略">
                    <Select
                      options={[
                        { value: "smart", label: "智能（余额优先）" },
                        { value: "round-robin", label: "轮询" },
                        { value: "drain", label: "集中消耗（低余额优先）" },
                      ]}
                    />
                  </Form.Item>
                </Col>
                <Col span={12}>
                  <Form.Item label="默认模型" tooltip="实时拉取 WorkBuddy 内置模型，上新后点右侧刷新即可看到">
                    <Space.Compact style={{ width: "100%" }}>
                      <Form.Item name="defaultModel" noStyle>
                        <Select
                          showSearch
                          style={{ width: "100%" }}
                          placeholder="选择模型"
                          optionFilterProp="label"
                          loading={modelsLoading}
                          options={modelOptions}
                          onChange={onModelChange}
                          notFoundContent={
                            modelsLoading ? "加载中…" : modelsError || "点右侧刷新获取模型"
                          }
                        />
                      </Form.Item>
                      <Tooltip title="刷新模型列表">
                        <Button
                          icon={<ReloadOutlined />}
                          loading={modelsLoading}
                          onClick={() => loadModels()}
                        />
                      </Tooltip>
                    </Space.Compact>
                  </Form.Item>
                  {modelsError && (
                    <div style={{ color: "#ff4d4f", fontSize: 12, marginTop: -18, marginBottom: 12 }}>
                      {modelsError}
                    </div>
                  )}
                </Col>
                <Col span={12}>
                  <Form.Item
                    name="contextWindow"
                    label="上下文窗口(tokens)"
                    extra={
                      selectedModel?.contextWindow
                        ? `所选模型上限 ${selectedModel.contextWindow.toLocaleString()}，可手动调低`
                        : undefined
                    }
                  >
                    <InputNumber style={{ width: "100%" }} min={1000} step={1000} />
                  </Form.Item>
                </Col>
                <Col span={12}>
                  <Form.Item name="reasoningEffort" label="默认思考强度">
                    <Select
                      options={[
                        { value: "none", label: "关闭" },
                        { value: "low", label: "低" },
                        { value: "medium", label: "中" },
                        { value: "high", label: "高" },
                      ]}
                    />
                  </Form.Item>
                </Col>
                <Col span={12}>
                  <Form.Item name="stickyTtlMin" label="会话粘性窗口(分钟)">
                    <InputNumber style={{ width: "100%" }} min={0} />
                  </Form.Item>
                </Col>
                <Col span={12}>
                  <Form.Item name="maxRetries" label="Failover 最大换号数">
                    <InputNumber style={{ width: "100%" }} min={1} max={10} />
                  </Form.Item>
                </Col>
              </Row>

              <Divider />
              <Button type="primary" loading={saving} onClick={onSave}>
                保存设置
              </Button>
            </Form>
          </Card>
        </Col>

        <Col xs={24} lg={10}>
          <Card title="接入信息" size="small" style={{ marginBottom: 16 }}>
            <Space direction="vertical" style={{ width: "100%" }}>
              <Text type="secondary">服务状态</Text>
              <Alert
                type={proxy?.running ? "success" : "info"}
                showIcon
                message={
                  proxy?.running
                    ? "反代运行中"
                    : settings.proxy.enabled
                      ? "已启用但未运行"
                      : "未启用"
                }
              />
              <Text type="secondary">Base URL</Text>
              <Space.Compact style={{ width: "100%" }}>
                <Input readOnly value={baseUrl} className="wm-mono" />
                <Tooltip title="复制">
                  <Button
                    icon={<CopyOutlined />}
                    onClick={() => copyText(baseUrl, "已复制 Base URL")}
                  />
                </Tooltip>
              </Space.Compact>
              <Text type="secondary" style={{ marginTop: 8 }}>
                API Key
              </Text>
              <Space.Compact style={{ width: "100%" }}>
                <Input.Password readOnly value={apiKey || ""} className="wm-mono" visibilityToggle />
                <Tooltip title="复制密钥">
                  <Button
                    icon={<CopyOutlined />}
                    onClick={() => copyText(apiKey || "", "已复制 API Key")}
                  />
                </Tooltip>
              </Space.Compact>
              <Text type="secondary" style={{ marginTop: 8 }}>
                默认模型
              </Text>
              <Space.Compact style={{ width: "100%" }}>
                <Input readOnly value={defaultModel || ""} className="wm-mono" placeholder="未选择" />
                <Tooltip title="复制模型名">
                  <Button
                    icon={<CopyOutlined />}
                    disabled={!defaultModel}
                    onClick={() => copyText(defaultModel || "", "已复制模型名")}
                  />
                </Tooltip>
              </Space.Compact>
              {selectedModel && (
                <Space size={4} wrap style={{ marginTop: 8 }}>
                  {selectedModel.supportsToolCall && <Tag>工具调用</Tag>}
                  {selectedModel.supportsImages && <Tag>图像</Tag>}
                  {selectedModel.supportsReasoning && <Tag>推理</Tag>}
                  {selectedModel.contextWindow > 0 && (
                    <Tag color="blue">上下文 {selectedModel.contextWindow.toLocaleString()}</Tag>
                  )}
                  {selectedModel.maxOutputTokens > 0 && (
                    <Tag color="green">输出 {selectedModel.maxOutputTokens.toLocaleString()}</Tag>
                  )}
                </Space>
              )}
              <Paragraph style={{ marginTop: 12, marginBottom: 0 }}>
                OpenAI 兼容端点：
                <br />
                <Text code className="wm-mono">
                  POST {baseUrl}/chat/completions
                </Text>
                <br />
                <Text code className="wm-mono">
                  GET {baseUrl}/models
                </Text>
              </Paragraph>
              <Paragraph type="secondary" style={{ marginTop: 8 }}>
                在 Claude Code / opencode 等 agent 中填写上述 Base URL 与密钥即可接入。
              </Paragraph>
            </Space>
          </Card>
        </Col>
      </Row>
    </div>
  );
}
