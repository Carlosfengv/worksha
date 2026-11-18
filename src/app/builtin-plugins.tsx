import { systemFeatureCatalog } from "@/features/plugins/system-features";
import { createProjectsPlugin } from "@/features/projects/plugin";
import { retryableLazy as lazy } from "./render-boundary";
import { createUiRegistry, type UiPlugin } from "./plugin-registry";

const AccountsPage = lazy(() =>
  import("@/features/iam/page").then((module) => ({
    default: module.AccountsPage,
  })),
);
const OverviewPage = lazy(() =>
  import("@/features/observation/overview-page").then((module) => ({
    default: module.OverviewPage,
  })),
);
const AgentsPage = lazy(() =>
  import("@/features/agents/page").then((module) => ({
    default: module.AgentsPage,
  })),
);
const ProvidersPage = lazy(() =>
  import("@/features/providers/page").then((module) => ({
    default: module.ProvidersPage,
  })),
);
const ModelServicesPage = lazy(() =>
  import("@/features/model-services/page").then((module) => ({
    default: module.ModelServicesPage,
  })),
);
const SettingsPage = lazy(() =>
  import("@/features/settings/page").then((module) => ({
    default: module.SettingsPage,
  })),
);
const GatewayPanel = lazy(() =>
  import("@/features/settings/gateway-panel").then((module) => ({
    default: module.GatewayPanel,
  })),
);
const PluginManager = lazy(() =>
  import("@/features/plugins/manager").then((module) => ({
    default: module.PluginManager,
  })),
);
const RequestsPage = lazy(() =>
  import("@/features/observation/requests-page").then((module) => ({
    default: module.RequestsPage,
  })),
);
const SessionsPage = lazy(() =>
  import("@/features/sessions/page").then((module) => ({
    default: module.SessionsPage,
  })),
);
const McpPage = lazy(() =>
  import("@/features/mcp/page").then((module) => ({ default: module.McpPage })),
);
const projectsPlugin = createProjectsPlugin(
  lazy(() =>
    import("@/features/projects/page").then(({ ProjectsPage }) => ({
      default: ProjectsPage,
    })),
  ),
);

export const builtinPlugins: readonly UiPlugin[] = [
  {
    id: "builtin.overview",
    version: 1,
    pages: [
      {
        id: "overview",
        label: "总览",
        icon: "home",
        footer: false,
        ownsBody: false,
        refreshInHeader: false,
        resetScrollOnObject: false,
        render: ({ data, navigate }) => (
          <OverviewPage data={data} navigate={navigate} />
        ),
      },
    ],
  },
  {
    id: "builtin.agents",
    version: 1,
    pages: [
      {
        id: "agents",
        label: "本机 Agents",
        icon: "brain",
        footer: false,
        ownsBody: false,
        refreshInHeader: false,
        resetScrollOnObject: true,
        render: ({ data, refresh, route }) => (
          <AgentsPage
            data={data}
            selectedId={route.objectId}
            selectedTab={route.tab}
            refresh={refresh}
          />
        ),
      },
    ],
  },
  {
    id: "builtin.providers",
    version: 1,
    pages: [
      {
        id: "providers",
        label: "模型与额度",
        icon: "globe",
        footer: false,
        ownsBody: false,
        refreshInHeader: false,
        resetScrollOnObject: true,
        render: ({ data, refresh, route, routeSearch, featureAvailable }) => (
          <ProvidersPage
            data={data}
            refresh={refresh}
            selectedId={route.objectId}
            selectedTab={route.tab}
            routeSearch={routeSearch}
            canCreateDefaultService={featureAvailable("model-services")}
          />
        ),
      },
    ],
  },
  {
    id: "builtin.model-services",
    version: 1,
    pages: [
      {
        id: "model-services",
        label: "模型服务与路由",
        icon: "globe",
        footer: false,
        ownsBody: false,
        refreshInHeader: false,
        resetScrollOnObject: true,
        render: ({ data, route }) => (
          <ModelServicesPage
            providers={data?.providers ?? []}
            selectedId={route.objectId}
            cloneFrom={route.from}
          />
        ),
      },
    ],
  },
  {
    id: "builtin.mcp",
    version: 1,
    pages: [
      {
        id: "mcp",
        label: "MCP",
        icon: "doc-thinking-steps",
        footer: false,
        ownsBody: true,
        refreshInHeader: false,
        resetScrollOnObject: false,
        render: ({ data, route, pageRefresh }) => (
          <McpPage
            selectedId={route.objectId}
            initialAgent={route.agent}
            initialDays={route.days}
            initialModel={route.model}
            initialProject={route.project}
            refreshToken={`${pageRefresh}:${JSON.stringify(data?.hiddenProjects ?? [])}`}
          />
        ),
      },
    ],
  },
  {
    id: "builtin.sessions",
    version: 1,
    pages: [
      {
        id: "sessions",
        label: "会话列表",
        icon: "message-circle",
        footer: false,
        ownsBody: true,
        refreshInHeader: false,
        resetScrollOnObject: false,
        render: ({ data, navigate, route, featureAvailable }) => (
          <SessionsPage
            projectsAvailable={featureAvailable("projects")}
            initialUnified={route.scope === "unified"}
            nativeOnly={route.scope === "local"}
            initialDomainSource={route.domainSource}
            data={data}
            navigate={navigate}
            selectedId={route.objectId}
            initialAgent={route.agent}
            anchor={route.anchor}
            returnAgent={route.from}
            returnTab={route.tab}
            returnTo={route.returnTo}
            initialView={route.view}
          />
        ),
      },
    ],
  },
  {
    id: "builtin.requests",
    version: 1,
    pages: [
      {
        id: "requests",
        label: "调用与工具",
        icon: "file-text",
        footer: false,
        ownsBody: true,
        refreshInHeader: false,
        resetScrollOnObject: false,
        render: ({ route, routeSearch }) => (
          <RequestsPage selectedId={route.objectId} routeSearch={routeSearch} />
        ),
      },
    ],
  },
  projectsPlugin,
  {
    id: "builtin.iam",
    version: 1,
    pages: [
      {
        id: "iam",
        label: "账号管理",
        icon: "shield",
        systemFeature: {
          purpose: "启用本机账号登录并按账号分配模型服务查看权限",
          scenes: ["accounts"],
          operations: [
            "管理账号和内置角色",
            "分配与撤回资源",
            "修改或重置密码",
          ],
          conditions:
            "默认关闭。完成首次设置后，此实例始终要求登录，账号管理插件无法关闭或卸载。",
          supportScenes: [],
        },
        render: () => <AccountsPage />,
      },
    ],
  },
  {
    id: "builtin.plugins",
    version: 1,
    pages: [
      {
        id: "plugins",
        label: "插件中心",
        icon: "square-library",
        footer: false,
        ownsBody: false,
        refreshInHeader: false,
        resetScrollOnObject: false,
        render: ({ data, route, routeSearch, refresh }) => (
          <PluginManager
            key={route.deferredPlugin ?? routeSearch}
            initialScene={
              route.deferredPlugin ??
              new URLSearchParams(routeSearch).get("scene") ??
              undefined
            }
            systemFeatures={systemFeatureCatalog(
              builtinPlugins.flatMap((plugin) =>
                plugin.pages.map((page) => ({
                  id: plugin.id,
                  page: page.id,
                  name: page.label,
                  details: page.systemFeature,
                })),
              ),
              data?.version,
            )}
            providers={data?.providers ?? []}
            refreshSnapshot={refresh}
          />
        ),
      },
    ],
  },
  {
    id: "builtin.settings",
    version: 1,
    pages: [
      {
        id: "lan-gateway",
        label: "常驻网关与密钥",
        icon: "lock",
        render: ({ data, route }) => (
          <GatewayPanel
            selectedId={route.objectId}
            providers={data?.providers ?? []}
            storeId={data?.settingsWriteProtocol?.storeId}
          />
        ),
      },
      {
        id: "settings",
        label: "系统设置",
        icon: "settings",
        footer: true,
        ownsBody: true,
        refreshInHeader: true,
        resetScrollOnObject: false,
        render: ({ data, refresh, route, featureAvailable }) => (
          <SettingsPage
            data={data}
            refresh={refresh}
            section={route.tab}
            accountSettings={
              featureAvailable("iam") ? <AccountsPage /> : undefined
            }
          />
        ),
      },
    ],
  },
];
export const pageRegistry = createUiRegistry(builtinPlugins);
