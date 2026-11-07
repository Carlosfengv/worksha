import type { PluginInfo } from "../../../contracts/plugins/builtin/types";
import { systemPluginPresentation, type PluginScenario } from "./scenarios";
export type SystemFeature = {
  id: string;
  name: string;
  href: string;
  version?: string;
  purpose: string;
  scenes: PluginScenario[];
  operations: string[];
  conditions: string;
  supportScenes: PluginScenario[];
};
export const businessDisableEffects: Record<string, string> = {
  "builtin.providers":
    "模型 API 连接、账户用量刷新与本机模型网关不可用；依赖它的模型服务、Agent 连接和任务同时停用。",
  "builtin.model-services":
    "服务创建、发布与已绑定服务的模型目录和调用不可用；已有 Agent 配置保留，其服务入口会报不可用。",
  "builtin.agents":
    "Agent 连接管理及经过本机 Agent 入口的模型调用不可用，任务同时停用；外部 Agent 自身继续运行。",
  "builtin.workbench":
    "本应用中的任务新建、续聊、审批与停止操作不可用；历史任务保留，不会终止外部 Agent。",
  "builtin.projects":
    "项目、修改、端口服务与磁盘/Git 查询不可用；任务页无法选择已有项目，但仍可选择本机目录。",
  "builtin.sessions":
    "会话读取与后台扫描停止，原生来源读取授权撤销；依赖它的项目、MCP 和总览同时停用。",
  "builtin.requests":
    "请求筛选、详情与导出不可用，总览同时停用；停用查看功能不会删除网关审计历史。",
  "builtin.overview":
    "活动与统计汇总不可用，实时活动采集停止，项目中的实时活动更新受影响；会话历史仍保留。",
  "builtin.mcp":
    "MCP 配置与调用观测不可用；外部 MCP 服务和工具进程不会被本应用终止。",
};
const definitions: Record<
  string,
  Omit<SystemFeature, "id" | "name" | "href" | "version">
> = {
  "model-services": {
    purpose: "把不同 Provider 的模型组织为可供 Agent 使用的服务",
    scenes: ["connections"],
    operations: ["创建模型服务", "编辑服务成员与路由策略", "删除服务"],
    conditions: "需要先配置 Provider 和可用模型；发布与切换以实际结果为准。",
    supportScenes: [],
  },
  tasks: {
    purpose: "创建和管理任务，在任务中继续对话并处理执行交互",
    scenes: ["tasks"],
    operations: ["新建任务", "连接设置", "发送", "停止本轮"],
    conditions:
      "需要可用的 Agent 连接和用户确认的项目目录。具体执行、审批及停止能力以所选 Agent 支持范围为准；读取 Codex 会话不等于已接入任务控制。",
    supportScenes: [],
  },
  projects: {
    purpose: "按会话整理项目，查看项目会话、修改、服务与磁盘信息",
    scenes: ["projects"],
    operations: [
      "查看项目",
      "Sessions",
      "修改记录",
      "端口与服务",
      "磁盘与 Git",
    ],
    conditions:
      "项目来自已授权会话目录。Git、文件与端口信息还取决于本机目录和读取权限；缺失信息保持未知。",
    supportScenes: ["projects"],
  },
  agents: {
    purpose: "管理 Agent 的模型连接和已有会话观测",
    scenes: ["connections", "activity"],
    operations: ["切换模型连接", "读取本机会话", "连接与观测"],
    conditions:
      "发现客户端不等于连接或授权成功；原生会话读取和模型连接分别配置。",
    supportScenes: ["activity"],
  },
  providers: {
    purpose: "管理模型 API 连接，以及原生账户和额度信息",
    scenes: ["connections", "accounts"],
    operations: ["添加供应商", "配置凭据与模型", "查看账户与额度"],
    conditions:
      "API 连接需要实际配置与验证；官方订阅与 API 额度不混为一项。具体数据以来源支持和授权范围为准。",
    supportScenes: ["accounts"],
  },
  mcp: {
    purpose: "查看 MCP 配置来源、工具调用记录和用量",
    scenes: ["activity"],
    operations: [
      "配置来源与历史关联",
      "筛选 MCP 调用",
      "查看调用详情",
      "MCP 调用比较",
    ],
    conditions:
      "当前页面用于配置与调用观测，不提供服务启动或工具执行。配置声明和实际调用记录分别展示，缺失状态保持未知。",
    supportScenes: [],
  },
  sessions: {
    purpose: "查看已授权来源的会话、调用时间线、上下文与原生用量",
    scenes: ["activity"],
    operations: [
      "搜索会话",
      "重新检查来源",
      "对话",
      "调用时间线",
      "上下文",
      "费用",
      "磁盘与项目",
    ],
    conditions:
      "需要读取对应原生来源或授权上传插件的数据；未提供的时序、费用与客户端归属保持未知。",
    supportScenes: ["activity"],
  },
  requests: {
    purpose: "筛选和查看已记录的模型请求及调用指标",
    scenes: ["activity"],
    operations: ["筛选请求", "打开请求详情", "导出记录"],
    conditions:
      "可见范围取决于实际采集和内容设置；未经观测入口的调用可能缺少指标，不代表完整调用量。",
    supportScenes: ["activity"],
  },
  overview: {
    purpose: "汇总已观测的 Agent 活动与模型调用趋势",
    scenes: ["activity"],
    operations: ["管理 Agents", "查看会话", "筛选统计来源与时间范围"],
    conditions:
      "活动、Token 速度、费用等以实际来源和采样能力为准；没有观测数据不能认作已验证空闲。",
    supportScenes: ["activity"],
  },
};
export function systemFeatureCatalog(
  pages: { id: string; page: string; name: string }[],
  version?: string,
): SystemFeature[] {
  return pages.flatMap((page) =>
    definitions[page.page]
      ? [
          {
            ...definitions[page.page],
            id: page.id,
            name: page.name,
            href: `#/${page.page}`,
            version,
          },
        ]
      : [],
  );
}

export function systemAgentPlugins(
  plugins: readonly PluginInfo[],
): SystemFeature[] {
  return plugins.flatMap((plugin) => {
    const source = plugin.capabilities.find(
      ({ address }) =>
        address.name === "session.source" && address.version === 1,
    );
    if (!source) return [];
    const presentation = systemPluginPresentation(plugin);
    return [
      {
        id: plugin.id,
        name: presentation.title,
        purpose: presentation.purpose,
        scenes: presentation.scenes,
        href: `#/agents/${encodeURIComponent(source.address.instance)}?tab=connection`,
        version: plugin.version,
        operations: ["读取本机会话", "查看连接与观测"],
        conditions:
          "启用后需单独授权读取。停用会停止本应用读取，Agent 客户端继续运行。",
        supportScenes: [],
      },
    ];
  });
}
