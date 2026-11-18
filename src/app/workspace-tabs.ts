import type { AppRoute } from "./plugin-registry";
import type { Page } from "./types";

export const homeSections = [
  ["观测", ["overview", "sessions", "requests", "projects"]],
  ["供给与网关", ["agents", "providers", "model-services", "lan-gateway"]],
] as const;

export type HomeSection = (typeof homeSections)[number][1][number];
const resourceKinds = {
  sessions: "session",
  requests: "request",
  mcp: "mcp",
  agents: "agent",
  providers: "provider",
  "model-services": "service",
  "lan-gateway": "lan-client",
  projects: "project",
} as const satisfies Partial<Record<Page, string>>;

export type WorkspaceResourceTab = {
  tabKey: string;
  kind:
    | (typeof resourceKinds)[keyof typeof resourceKinds]
    | "extension"
    | "plugins"
    | "settings";
  objectId: string;
  ownerPage: Page;
  title: string;
  routeHash: string;
  pinned: boolean;
  dirty?: boolean;
};

export type WorkspaceTabs = {
  activeTabKey: string;
  homeSection: HomeSection;
  homePage: Page;
  sectionSearch: Partial<Record<Page, string>>;
  tabs: WorkspaceResourceTab[];
};

export const initialWorkspace: WorkspaceTabs = {
  activeTabKey: "home",
  homeSection: "overview",
  homePage: "overview",
  sectionSearch: {},
  tabs: [],
};

export function homeSection(page: Page): HomeSection {
  if (page === "mcp") return "requests";
  return homeSections.some(([, sections]) =>
    (sections as readonly Page[]).includes(page),
  )
    ? (page as HomeSection)
    : "overview";
}

export function resourceTab(
  route: AppRoute,
  routeHash: string,
  title?: string,
): WorkspaceResourceTab | undefined {
  const { page, objectId, extensionPlugin, extensionView } = route;
  const kind = extensionPlugin
    ? "extension"
    : page === "plugins" || page === "settings"
      ? page
      : page === "iam"
        ? "settings"
        : objectId
          ? resourceKinds[page as keyof typeof resourceKinds]
          : undefined;
  if (!kind) return undefined;
  const identity = extensionPlugin
    ? JSON.stringify([extensionPlugin, extensionView])
    : (objectId ?? page);
  return {
    tabKey: `${kind}:${identity}`,
    kind,
    objectId: identity,
    ownerPage: page,
    title:
      title ??
      (kind === "request"
        ? `#${identity}`
        : kind === "settings"
          ? page === "iam"
            ? "账号管理"
            : "系统设置"
          : kind === "plugins"
            ? "插件中心"
            : identity),
    routeHash,
    pinned: ["plugins", "settings", "extension"].includes(kind),
  };
}

const maxPinned = 12;

function makeRoom(state: WorkspaceTabs, excluding?: string) {
  if (state.tabs.filter((tab) => tab.pinned).length < maxPinned)
    return state.tabs;
  const drop = state.tabs.find(
    (tab) =>
      tab.pinned &&
      !tab.dirty &&
      tab.tabKey !== state.activeTabKey &&
      tab.tabKey !== excluding,
  );
  return drop ? state.tabs.filter((tab) => tab !== drop) : undefined;
}

export function openResource(
  state: WorkspaceTabs,
  input: WorkspaceResourceTab,
  pin = input.pinned,
): WorkspaceTabs {
  const existing = state.tabs.find((tab) => tab.tabKey === input.tabKey);
  if (existing) {
    const activated = {
      ...state,
      activeTabKey: existing.tabKey,
      tabs: state.tabs.map((tab) =>
        tab === existing ? { ...tab, routeHash: input.routeHash } : tab,
      ),
    };
    return pin ? pinTab(activated, input.tabKey) : activated;
  }

  if (pin) {
    const tabs = makeRoom(state);
    if (!tabs) return state;
    return {
      ...state,
      tabs: [...tabs, { ...input, pinned: true }],
      activeTabKey: input.tabKey,
    };
  }

  const preview = state.tabs.find((tab) => !tab.pinned);
  return {
    ...state,
    activeTabKey: input.tabKey,
    tabs: preview
      ? state.tabs.map((tab) =>
          tab === preview ? { ...input, pinned: false } : tab,
        )
      : [...state.tabs, { ...input, pinned: false }],
  };
}

export function openHome(
  state: WorkspaceTabs,
  page?: Page,
  search?: string,
): WorkspaceTabs {
  return {
    ...state,
    activeTabKey: "home",
    homeSection: page ? homeSection(page) : state.homeSection,
    homePage: page ?? state.homePage,
    sectionSearch:
      page && search !== undefined
        ? { ...state.sectionSearch, [page]: search }
        : state.sectionSearch,
  };
}

export function homeHash(state: WorkspaceTabs, page = state.homePage) {
  const search = state.sectionSearch[page];
  return `#/${page}${search ? `?${search}` : ""}`;
}

export function activeHash(state: WorkspaceTabs) {
  return (
    state.tabs.find((tab) => tab.tabKey === state.activeTabKey)?.routeHash ??
    homeHash(state)
  );
}

export function pinTab(state: WorkspaceTabs, tabKey: string): WorkspaceTabs {
  const tab = state.tabs.find((tab) => tab.tabKey === tabKey);
  if (!tab || tab.pinned) return state;
  const tabs = makeRoom(state, tabKey);
  if (!tabs) return state;
  return {
    ...state,
    tabs: tabs.map((item) => (item === tab ? { ...item, pinned: true } : item)),
  };
}

export function setTabDirty(
  state: WorkspaceTabs,
  tabKey: string,
  dirty: boolean,
): WorkspaceTabs {
  const tab = state.tabs.find((item) => item.tabKey === tabKey);
  if (!tab || (!!tab.dirty === dirty && (!dirty || tab.pinned))) return state;
  const pinned = dirty ? pinTab(state, tabKey) : state;
  // Keep the draft even if every pinned slot is in use by another draft.
  return {
    ...pinned,
    tabs: pinned.tabs.map((item) =>
      item.tabKey === tabKey
        ? { ...item, dirty, pinned: item.pinned || dirty }
        : item,
    ),
  };
}

export function closeTabs(
  state: WorkspaceTabs,
  keys: readonly string[],
  goHome = false,
): WorkspaceTabs {
  const keep = (tab: WorkspaceResourceTab) => !keys.includes(tab.tabKey);
  const tabs = state.tabs.filter(keep);
  const index = state.tabs.findIndex(
    (tab) => tab.tabKey === state.activeTabKey,
  );
  const activeTabKey = goHome
    ? "home"
    : index >= 0 && !keep(state.tabs[index])
      ? ((
          state.tabs.slice(index + 1).find(keep) ??
          state.tabs.slice(0, index).findLast(keep)
        )?.tabKey ?? "home")
      : state.activeTabKey;
  return { ...state, tabs, activeTabKey };
}

export function pruneUnavailableTabs(
  state: WorkspaceTabs,
  available: (tab: WorkspaceResourceTab) => boolean,
): WorkspaceTabs {
  const invalid = state.tabs.filter(
    (tab) => tab.tabKey !== state.activeTabKey && !available(tab),
  );
  return invalid.length
    ? closeTabs(
        state,
        invalid.map((tab) => tab.tabKey),
      )
    : state;
}
