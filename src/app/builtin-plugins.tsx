import { systemFeatureCatalog } from "@/features/plugins/system-features";
import { retryableLazy as lazy } from "./render-boundary";
import type { PageContext } from "./plugin-registry";
import { createUiRegistry, type UiPlugin } from "./plugin-registry";

const OverviewPage = lazy(() =>
  import("@/features/observation/overview-page").then((module) => ({
    default: module.OverviewPage,
  })),
);
const ExtensionsPage = lazy(() =>
  import("@/features/plugins/page").then((module) => ({
    default: module.ExtensionsPage,
  })),
);
export function renderExtension({ route, data, refresh }: PageContext) {
  return (
    <ExtensionsPage
      key={`${route.extensionPlugin}:${route.extensionView}`}
      pluginId={route.extensionPlugin!}
      viewId={route.extensionView!}
      providers={data?.providers ?? []}
      refreshSnapshot={refresh}
    />
  );
}
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
        label: "Agents",
        icon: "brain",
        footer: false,
        ownsBody: false,
        refreshInHeader: true,
        resetScrollOnObject: false,
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
        label: "Providers",
        icon: "globe",
        footer: false,
        ownsBody: false,
        refreshInHeader: true,
        resetScrollOnObject: true,
        render: ({ data, refresh, route }) => (
          <ProvidersPage
            data={data}
            refresh={refresh}
            selectedId={route.objectId}
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
        label: "模型服务",
        icon: "globe",
        footer: false,
        ownsBody: false,
        refreshInHeader: false,
        resetScrollOnObject: false,
        render: ({ data }) => (
          <ModelServicesPage providers={data?.providers ?? []} />
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
        ownsBody: false,
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
        label: "Sessions",
        icon: "message-circle",
        footer: false,
        ownsBody: true,
        refreshInHeader: false,
        resetScrollOnObject: false,
        render: ({ data, navigate, route }) => (
          <SessionsPage
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
        label: "请求",
        icon: "file-text",
        footer: false,
        ownsBody: false,
        refreshInHeader: false,
        resetScrollOnObject: false,
        render: ({ route, routeSearch }) => (
          <RequestsPage selectedId={route.objectId} routeSearch={routeSearch} />
        ),
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
        id: "settings",
        label: "设置",
        icon: "settings",
        footer: true,
        ownsBody: false,
        refreshInHeader: true,
        resetScrollOnObject: false,
        render: ({ data, refresh }) => (
          <SettingsPage data={data} refresh={refresh} />
        ),
      },
    ],
  },
];
export const pageRegistry = createUiRegistry(builtinPlugins);
