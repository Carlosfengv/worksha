import type { ReactNode } from "react";
import type { Page, Snapshot } from "./types";
import type { IconName } from "@/lib/icon-context";

export interface AppRoute {
  extensionPlugin?: string;
  extensionView?: string;
  domainSource?: string;
  page: Page;
  objectId?: string;
  agent?: string;
  anchor?: string;
  from?: string;
  tab?: string;
  days?: string;
  model?: string;
  project?: string;
  returnTo?: string;
  scope?: string;
  view?: "context" | "requests";
}
export interface PageContext {
  route: AppRoute;
  routeSearch: string;
  data: Snapshot | null;
  refresh: () => Promise<void>;
  navigate: (page: Page) => void;
  pageRefresh: number;
}
export interface PageContribution {
  id: Page;
  label: string;
  icon: IconName;
  footer?: boolean;
  ownsBody?: boolean;
  refreshInHeader?: boolean;
  resetScrollOnObject?: boolean;
  render: (context: PageContext) => ReactNode;
}
export interface UiPlugin {
  id: string;
  version: 1;
  pages: readonly PageContribution[];
}
const coreRoutes: readonly Page[] = [
  "overview",
  "agents",
  "projects",
  "tasks",
  "providers",
  "model-services",
  "mcp",
  "sessions",
  "requests",
  "plugins",
  "settings",
];

export function createUiRegistry(plugins: readonly UiPlugin[]) {
  const ids = new Set<string>();
  const pages = new Map<Page, Readonly<PageContribution>>();
  for (const plugin of plugins) {
    if (!plugin.id || ids.has(plugin.id) || plugin.version !== 1)
      throw new Error("页面插件描述无效或重复");
    ids.add(plugin.id);
    for (const page of plugin.pages) {
      if (!coreRoutes.includes(page.id) || pages.has(page.id) || !page.label)
        throw new Error("页面贡献无效或冲突");
      pages.set(page.id, Object.freeze({ ...page }));
    }
  }
  if (coreRoutes.some((id) => !pages.has(id)))
    throw new Error("现有页面贡献缺失");
  const navigation = Object.freeze([...pages.values()]);
  return Object.freeze({
    navigation,
    resolve: (id: Page) => pages.get(id)!,
    readRoute(hash: string): AppRoute {
      const [path, search] = hash.replace(/^#\/?/, "").split("?");
      const extension = path.split("/");
      if (extension[0] === "extensions" && extension.length === 3) {
        try {
          const plugin = decodeURIComponent(extension[1]);
          const view = decodeURIComponent(extension[2]);
          if (
            /^[a-z0-9.-]{5,100}$/.test(plugin) &&
            /^[a-z0-9_-]{1,64}$/.test(view)
          )
            return {
              page: "plugins",
              extensionPlugin: plugin,
              extensionView: view,
            };
        } catch {
          /* Invalid deep links use the existing safe fallback. */
        }
      }
      const [page, rawId] = path.split("/");
      const params = new URLSearchParams(search);
      let objectId: string | undefined;
      try {
        objectId = rawId ? decodeURIComponent(rawId) : undefined;
      } catch {
        objectId = undefined;
      }
      return {
        page: pages.has(page as Page) ? (page as Page) : "overview",
        objectId,
        agent: params.get("agent") ?? undefined,
        anchor: params.get("anchor") ?? undefined,
        from: params.get("from") ?? undefined,
        tab: params.get("tab") ?? undefined,
        days: params.get("days") ?? undefined,
        model: params.get("model") ?? undefined,
        project: params.get("project") ?? undefined,
        returnTo: params.get("returnTo") ?? undefined,
        scope: params.get("scope") ?? undefined,
        domainSource: params.get("source") ?? undefined,
        view:
          params.get("view") === "context"
            ? "context"
            : params.get("view") === "requests"
              ? "requests"
              : undefined,
      };
    },
  });
}
