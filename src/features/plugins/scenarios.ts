import type { PluginInfo } from "../../../contracts/plugins/builtin/types";
import type { Manifest } from "./types";
export const pluginScenarios = {
  agents: {
    title: "Agent 接入",
    description: "接入 Agent 的会话、调用与活动，具体可用功能由各插件提供。",
    href: "#/agents",
    action: "打开 Agents",
  },
  tasks: {
    title: "任务执行",
    description: "创建任务、继续对话并处理任务执行。",
    href: "#/tasks",
    action: "打开任务",
  },
  activity: {
    title: "会话与观测",
    description:
      "查看 Agent 已有会话、调用记录与当前活动。原生记录需单独授权读取。",
    href: "#/sessions",
    action: "打开会话",
  },
  projects: {
    title: "项目管理",
    description: "整理项目，查看文件统计、版本库状态和关联的本地服务。",
    href: "#/projects",
    action: "打开项目",
  },
  accounts: {
    title: "账户与用量",
    description:
      "查看已接入账户的订阅、额度与用量；具体覆盖以账户返回结果为准。",
    href: "#/providers",
    action: "管理账户",
  },
  connections: {
    title: "模型与工具",
    description: "配置模型连接、模型服务与路由，扩展接口适配和连接预设。",
    href: "#/model-services",
    action: "打开模型服务",
  },
  reports: {
    title: "报表与界面",
    description: "添加统计报表、对象详情信息和扩展展示。",
    href: null,
    action: null,
  },
  other: {
    title: "其他功能",
    description: "具体能力和配置方式见插件详情。",
    href: null,
    action: null,
  },
} as const;
export type PluginScenario = keyof typeof pluginScenarios;
export const isPluginScenario = (value: string): value is PluginScenario =>
  Object.hasOwn(pluginScenarios, value);
const systemNames: Record<string, { title: string; purpose: string }> = {
  "io.localagent.builtin.source.claude": {
    title: "Claude 接入",
    purpose: "查看本机已有对话、工具记录和原生用量",
  },
  "io.localagent.builtin.source.codex": {
    title: "Codex 接入",
    purpose: "查看 Codex CLI 与桌面端共享的已有会话",
  },
  "io.localagent.builtin.source.opencode": {
    title: "OpenCode 接入",
    purpose: "查看本机已有对话、工具记录和原生用量",
  },
  sessions: { title: "自动更新会话", purpose: "发现新增会话并更新历史列表" },
  activity: { title: "实时活动", purpose: "查看 Agent 当前任务的活动进展" },
  listeners: {
    title: "自动发现本地服务",
    purpose: "更新项目关联的本地监听端口",
  },
  "project.source": { title: "项目列表", purpose: "按会话记录整理项目与目录" },
  "project.assets": { title: "项目统计", purpose: "查看项目内文件和使用统计" },
  "project.git": { title: "版本库状态", purpose: "查看项目的 Git 状态" },
  "project.listeners": {
    title: "项目服务",
    purpose: "查看项目关联的本地服务端口",
  },
};
export function systemPluginPresentation(plugin: PluginInfo) {
  const scenes = new Set<PluginScenario>();
  for (const cap of plugin.capabilities) {
    const { name, instance } = cap.address;
    if (name === "session.source") scenes.add("agents");
    if (
      name.startsWith("session.") ||
      (name === "collector" && ["sessions", "activity"].includes(instance))
    )
      scenes.add("activity");
    if (
      name.startsWith("project.") ||
      (name === "collector" && instance === "listeners")
    )
      scenes.add("projects");
    if (name === "account.source") scenes.add("accounts");
  }
  const account = plugin.capabilities.find(
    (c) => c.address.name === "account.source",
  );
  const known = systemNames[plugin.id];
  return {
    title:
      known?.title ??
      (account
        ? plugin.displayName.replace(/账户读取$/, "账户与用量")
        : plugin.displayName),
    purpose:
      known?.purpose ??
      (account
        ? "查看已授权账户的订阅、额度或用量信息"
        : "具体能力和配置方式见详情"),
    scenes: scenes.size ? [...scenes] : ["other" as PluginScenario],
  };
}
const purposes: Record<string, string> = {
  "session.source@1": "读取会话记录",
  "account.quota-source@1": "查看账户额度",
  "provider.catalog@1": "添加模型连接预设",
  "provider.models@1": "发现可用模型",
  "ui.contribution@1": "扩展界面展示",
  "report.view@1": "查看统计报表",
  "gateway.protocol@1": "适配模型接口",
  "config.plan-provider@1": "管理连接配置",
  "integration.connector@1": "连接工具服务",
  "domain.provider-access@1": "扩展模型连接",
};
export function uploadedPluginPresentation(manifest: Manifest) {
  const caps = manifest.capabilities;
  const scenes = new Set<PluginScenario>();
  if (manifest.agent || caps.includes("domain.agent-directory@1"))
    scenes.add("agents");
  if (
    caps.some((c) =>
      [
        "session.source@1",
        "domain.agent-directory@1",
        "domain.session-source@1",
        "domain.call-source@1",
        "domain.activity-source@1",
        "domain.mcp-source@1",
      ].includes(c),
    )
  )
    scenes.add("activity");
  if (caps.includes("domain.project-source@1")) scenes.add("projects");
  if (caps.includes("account.quota-source@1")) scenes.add("accounts");
  if (
    caps.some((c) =>
      [
        "provider.catalog@1",
        "provider.models@1",
        "domain.provider-access@1",
        "gateway.protocol@1",
        "config.plan-provider@1",
        "integration.connector@1",
        "domain.routing-policy@1",
      ].includes(c),
    )
  )
    scenes.add("connections");
  if (
    caps.includes("report.view@1") ||
    (!scenes.size && caps.includes("ui.contribution@1"))
  )
    scenes.add("reports");
  if (!scenes.size) scenes.add("other");
  const descriptions = caps
    .filter((c) => c !== "ui.contribution@1" || caps.length === 1)
    .map((c) => purposes[c])
    .filter(Boolean);
  return {
    scenes: [...scenes],
    purpose: descriptions.length
      ? descriptions.join("、")
      : scenes.has("activity")
        ? "读取插件提供的会话、调用与活动记录"
        : scenes.has("connections")
          ? "扩展模型连接与选择"
          : "具体能力和配置方式见详情",
  };
}
