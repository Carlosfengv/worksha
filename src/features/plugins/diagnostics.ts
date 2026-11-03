import { object } from "./contract";

export const capabilityDetails: Record<
  string,
  { name: string; scope: string }
> = {
  "session.source@1": {
    name: "原生历史读取",
    scope: "只读所选单个 JSONL，最多 8 MiB、10240 条记录；正文按需读取。",
  },
  "account.quota-source@1": {
    name: "账户额度采集",
    scope: "向明确绑定的连接执行限定 GET；原始凭据由宿主持有。",
  },
  "provider.catalog@1": {
    name: "供应商预设",
    scope: "复制为本机连接后，仍需配置和验证；预设本身不代表已连接。",
  },
  "provider.models@1": {
    name: "只读模型目录",
    scope:
      "仅向明确授权的连接范围发送目录 GET；原始凭据由宿主持有，不执行模型调用。",
  },
  "ui.contribution@1": {
    name: "宿主扩展视图",
    scope: "使用宿主提供的界面；包不能执行前端脚本。",
  },
  "report.view@1": {
    name: "网关汇总报表",
    scope: "单独授权后读取最近 24 小时统一网关摘要。",
  },
  "gateway.protocol@1": {
    name: "协议适配",
    scope: "仅用于明确绑定的已有连接；请求、网络和凭据由宿主管理。",
  },
  "config.plan-provider@1": {
    name: "有限配置计划",
    scope: "针对所选 JSON 文件预览有限字段；确认、备份和恢复由宿主执行。",
  },
  "integration.connector@1": {
    name: "MCP 连接与观测",
    scope: "明确启动所选 stdio 或 HTTP 连接；管理页不主动执行工具。",
  },
};
export const snapshotNames: Record<string, string> = {
  "not-applicable": "此能力不产生采集快照",
  disabled: "插件已停用，快照不可用",
  "awaiting-authorization": "等待资源授权",
  "not-read": "尚无本次启动的完整读取结果",
  available: "上次完整快照可用",
  "confirmed-empty": "上次完整读取确认无记录",
  stale: "保留上次完整结果，当前已过期或读取失败",
};
export const operationNames: Record<string, string> = {
  enable: "启用",
  disable: "停用",
  remove: "卸载",
  rollback: "版本回退",
  "revoke-grants": "撤销资源授权",
  "revoke-publisher": "撤销发布者信任",
  refresh: "刷新采集",
  "read-session": "读取会话正文",
  "authorize-report": "授权报表",
  "bind-connection": "绑定额度连接",
  "save-settings": "保存设置",
  "create-provider": "创建连接",
  "bind-protocol": "绑定协议",
  "unbind-protocol": "解除协议绑定",
  "prepare-config": "预览配置",
  "apply-config": "应用配置",
  "restore-config": "恢复配置",
  "start-connector": "启动 MCP 连接",
  "stop-connector": "停止 MCP 连接",
  "refresh-connector": "读取 MCP 状态",
};
export interface PluginActivity {
  since: string;
  attempts: string;
  succeeded: string;
  failed: string;
  interrupted: string;
  timeouts: string;
  resourceLimits: string;
  workerCrashes: string;
  inFlight: string;
  lastOperation: string | null;
  lastFinishedAt: string | null;
  lastSuccessAt: string | null;
  lastErrorCode: string | null;
  lastDurationMs: string | null;
}
export interface PluginDiagnostic {
  pluginId: string;
  digest: string;
  activity: PluginActivity | null;
  snapshot: {
    state: string;
    observedAt: string | null;
    records: string | null;
  };
}
export interface PluginDiagnostics {
  version: 1;
  available: true;
  scope: "service-lifetime";
  since: string;
  dropped: string;
  items: PluginDiagnostic[];
}
const decimal = (v: unknown) =>
  typeof v === "string" &&
  /^(0|[1-9][0-9]{0,19})$/.test(v) &&
  BigInt(v) <= 18446744073709551615n;
const timestamp = (v: unknown) =>
  typeof v === "string" && v.length <= 80 && Number.isFinite(Date.parse(v));
const nullable = (v: unknown, check: (value: unknown) => boolean) =>
  v === null || check(v);
export function parseDiagnostics(value: unknown): PluginDiagnostics {
  if (
    !object(value) ||
    value.version !== 1 ||
    value.available !== true ||
    value.scope !== "service-lifetime" ||
    !timestamp(value.since) ||
    !decimal(value.dropped) ||
    !Array.isArray(value.items) ||
    value.items.length > 64
  ) {
    throw new Error("运行诊断暂不可用；已安装插件仍可管理。");
  }
  const keys = new Set<string>();
  for (const item of value.items) {
    if (
      !object(item) ||
      typeof item.pluginId !== "string" ||
      item.pluginId.length > 100 ||
      typeof item.digest !== "string" ||
      !/^[a-f0-9]{64}$/.test(item.digest) ||
      keys.has(`${item.pluginId}:${item.digest}`) ||
      !object(item.snapshot) ||
      typeof item.snapshot.state !== "string" ||
      !Object.hasOwn(snapshotNames, item.snapshot.state) ||
      !nullable(item.snapshot.observedAt, timestamp) ||
      !nullable(item.snapshot.records, decimal)
    ) {
      throw new Error("运行诊断响应无效；已安装插件仍可管理。");
    }
    keys.add(`${item.pluginId}:${item.digest}`);
    const a = item.activity;
    if (a === null) continue;
    if (
      !object(a) ||
      !timestamp(a.since) ||
      ![
        "attempts",
        "succeeded",
        "failed",
        "interrupted",
        "timeouts",
        "resourceLimits",
        "workerCrashes",
        "inFlight",
      ].every((k) => decimal(a[k])) ||
      !nullable(
        a.lastOperation,
        (v) => typeof v === "string" && Object.hasOwn(operationNames, v),
      ) ||
      !nullable(a.lastFinishedAt, timestamp) ||
      !nullable(a.lastSuccessAt, timestamp) ||
      !nullable(a.lastDurationMs, decimal) ||
      !nullable(
        a.lastErrorCode,
        (v) => typeof v === "string" && /^[a-z][a-z0-9_]{0,63}$/.test(v),
      )
    ) {
      throw new Error("运行诊断响应无效；已安装插件仍可管理。");
    }
  }
  return value as unknown as PluginDiagnostics;
}
