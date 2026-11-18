import { useEffect, useRef, useState } from "react";
import { AppShellHeader } from "@/components/ui/app-shell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import {
  DropdownContent,
  DropdownMenu,
  DropdownTrigger,
} from "@/components/ui/dropdown";
import { MenuItem } from "@/components/ui/menu-item";
import { Kbd } from "@/components/ui/kbd";
import {
  NavItem,
  NavItemAction,
  NavItemContent,
  NavItemLabel,
  NavItemLeading,
  NavItemTrigger,
} from "@/components/ui/nav-item";
import { SidebarAccountMenu } from "@/components/ui/sidebar-account-menu";
import {
  TopNav,
  TopNavActions,
  TopNavBrand,
  TopNavNavigation,
} from "@/components/ui/top-nav";
import { useIam } from "@/features/iam/context";
import { useIamAction } from "@/features/iam/actions";
import { useIcon, type IconName } from "@/lib/icon-context";
import { pageRegistry } from "./builtin-plugins";
import type { WorkspaceTabs } from "./workspace-tabs";

type Theme = "system" | "light" | "dark";
const themeKey = "local-agent.theme";

function SystemMenu({ open }: { open: (href: string) => void }) {
  const { status, logout } = useIam();
  const action = useIamAction();
  const Settings = useIcon("settings");
  const [theme, setTheme] = useState<Theme>(() => {
    const saved = localStorage.getItem(themeKey);
    return saved === "light" || saved === "dark" ? saved : "system";
  });
  useEffect(() => {
    const scheme = matchMedia("(prefers-color-scheme: dark)");
    const apply = () =>
      document.documentElement.classList.toggle(
        "dark",
        theme === "dark" || (theme === "system" && scheme.matches),
      );
    apply();
    scheme.addEventListener("change", apply);
    localStorage.setItem(themeKey, theme);
    return () => scheme.removeEventListener("change", apply);
  }, [theme]);

  if (status?.enabled && status.account) {
    return (
      <div className="max-w-40">
        <SidebarAccountMenu
          primary={status.account.displayName}
          description={status.account.username}
          menuSide="bottom"
          menuAlign="end"
          sections={[
            {
              items: [
                {
                  id: "plugins",
                  label: "插件中心",
                  onSelect: () => open("#/plugins"),
                },
                {
                  id: "settings",
                  label: "系统与账号设置",
                  onSelect: () => open("#/settings"),
                },
              ],
            },
            {
              items: [
                {
                  id: "logout",
                  label: "退出登录",
                  disabled: action.busy,
                  onSelect: () =>
                    void action.run(async () => {
                      await logout?.();
                    }),
                },
              ],
            },
          ]}
        />
        {action.error && (
          <span role="alert" className="sr-only">
            {action.error}
          </span>
        )}
      </div>
    );
  }

  return (
    <DropdownMenu>
      <DropdownTrigger
        render={
          <Button
            variant="ghost"
            size="sm"
            aria-label="设置与扩展"
            leadingIcon={Settings}
          />
        }
      />
      <DropdownContent align="end">
        <MenuItem
          index={0}
          label="插件中心"
          onSelect={() => open("#/plugins")}
        />
        <MenuItem
          index={1}
          label="系统设置"
          onSelect={() => open("#/settings")}
        />
        <MenuItem
          index={2}
          label={`主题 · ${{ system: "跟随系统", light: "浅色", dark: "深色" }[theme]}`}
          onSelect={() =>
            setTheme(
              theme === "system"
                ? "light"
                : theme === "light"
                  ? "dark"
                  : "system",
            )
          }
        />
      </DropdownContent>
    </DropdownMenu>
  );
}

export function WorkspaceTitlebar({
  workspace,
  extensions,
  activate,
  pin,
  close,
  open,
  refresh,
  loading,
  status,
}: {
  workspace: WorkspaceTabs;
  extensions: readonly {
    id: string;
    href: string;
    label: string;
    icon: IconName;
  }[];
  activate: (key: string) => void;
  pin: (key: string) => void;
  close: (keys: string[], goHome?: boolean) => void;
  open: (href: string) => void;
  refresh: () => void;
  loading: boolean;
  status: string;
}) {
  const Home = useIcon("home");
  const Pin = useIcon("pin");
  const Close = useIcon("x");
  const More = useIcon("ellipsis");
  const Search = useIcon("search");
  const Refresh = useIcon("rotate-ccw");
  const [command, setCommand] = useState(false);
  const activeElement = useRef<HTMLElement>(null);
  const { activeTabKey } = workspace;
  const tabs = workspace.tabs.map((tab) => ({
    ...tab,
    title:
      extensions.find((entry) => entry.href === tab.routeHash)?.label ??
      tab.title,
  }));
  useEffect(() => {
    activeElement.current?.scrollIntoView({
      block: "nearest",
      inline: "nearest",
    });
  }, [activeTabKey]);
  useEffect(() => {
    const shortcut = (event: KeyboardEvent) => {
      if (
        !(event.metaKey || event.ctrlKey) ||
        event.altKey ||
        event.shiftKey ||
        event.defaultPrevented ||
        event.isComposing
      )
        return;
      const key = event.key.toLowerCase();
      if (key === "k") {
        event.preventDefault();
        setCommand((visible) => !visible);
      } else if (key === "1") {
        event.preventDefault();
        activate("home");
      } else if (key === "w" && activeTabKey !== "home") {
        if (
          !event.metaKey &&
          event.target instanceof Element &&
          event.target.closest(
            "input,textarea,select,[contenteditable]:not([contenteditable=false])",
          )
        )
          return;
        event.preventDefault();
        close([activeTabKey]);
      }
    };
    window.addEventListener("keydown", shortcut);
    return () => window.removeEventListener("keydown", shortcut);
  }, [activate, activeTabKey, close]);

  return (
    <AppShellHeader aria-label="窗口工具栏" className="shrink-0">
      <TopNav navigationAlign="left" className="h-11 min-h-0 gap-1 px-0 pr-2">
        <TopNavBrand className="h-full gap-0 px-0">
          <div data-tauri-drag-region className="h-full w-21 shrink-0" />
          <NavItem value="home" active={activeTabKey === "home"}>
            <NavItemTrigger
              className="h-8 gap-1.5"
              render={<button type="button" />}
              onClick={() => activate("home")}
              aria-label="Home"
            >
              <Home size={16} /> Home <Kbd className="max-sm:hidden">⌘1</Kbd>
            </NavItemTrigger>
          </NavItem>
        </TopNavBrand>
        <TopNavNavigation className="gap-1">
          <nav
            aria-label="工作区标签"
            className="flex min-w-0 items-center gap-1 overflow-x-auto [scrollbar-width:none]"
          >
            {tabs.map((tab) => (
              <NavItem
                key={tab.tabKey}
                value={tab.tabKey}
                active={tab.tabKey === activeTabKey}
                ref={tab.tabKey === activeTabKey ? activeElement : undefined}
                data-workspace-tab={tab.tabKey}
                data-pinned={tab.pinned}
                className="w-max max-w-56 shrink-0"
              >
                <NavItemTrigger
                  className="h-8"
                  render={<button type="button" />}
                  title={tab.title}
                  onClick={() => activate(tab.tabKey)}
                  onDoubleClick={() => pin(tab.tabKey)}
                >
                  {tab.dirty && (
                    <span
                      aria-label="有未发布变更"
                      className="text-status-warning"
                    >
                      ●
                    </span>
                  )}
                  <NavItemContent>
                    <NavItemLabel className={tab.pinned ? undefined : "italic"}>
                      {tab.title}
                    </NavItemLabel>
                  </NavItemContent>
                </NavItemTrigger>
                {!tab.pinned && (
                  <NavItemAction
                    title="固定标签"
                    aria-label={`固定标签 ${tab.title}`}
                    leadingIcon={Pin}
                    onClick={() => pin(tab.tabKey)}
                  />
                )}
                <NavItemAction
                  title="关闭标签"
                  aria-label={`关闭标签 ${tab.title}`}
                  leadingIcon={Close}
                  onClick={() => close([tab.tabKey])}
                />
              </NavItem>
            ))}
          </nav>
          {tabs.length > 0 && (
            <DropdownMenu>
              <DropdownTrigger
                render={
                  <Button
                    variant="ghost"
                    size="sm"
                    aria-label="标签操作"
                    leadingIcon={More}
                  />
                }
              />
              <DropdownContent>
                {activeTabKey !== "home" && tabs.length > 1 && (
                  <MenuItem
                    index={0}
                    label="关闭其他动态标签"
                    onSelect={() =>
                      close(
                        tabs
                          .filter(({ tabKey }) => tabKey !== activeTabKey)
                          .map(({ tabKey }) => tabKey),
                      )
                    }
                  />
                )}
                <MenuItem
                  index={1}
                  label="关闭全部动态标签并回 Home"
                  onSelect={() =>
                    close(
                      tabs.map(({ tabKey }) => tabKey),
                      true,
                    )
                  }
                />
                {tabs.map((tab, index) => (
                  <MenuItem
                    key={tab.tabKey}
                    index={index + 2}
                    label={tab.title}
                    onSelect={() => activate(tab.tabKey)}
                  />
                ))}
              </DropdownContent>
            </DropdownMenu>
          )}
          <div
            data-tauri-drag-region
            aria-label="窗口拖拽区"
            className="h-full min-w-16 flex-1"
          />
        </TopNavNavigation>
        <TopNavActions className="px-0">
          <Button
            variant="ghost"
            size="sm"
            leadingIcon={Search}
            aria-label="搜索与切换标签"
            onClick={() => setCommand(true)}
          >
            <Kbd className="max-sm:hidden">⌘K</Kbd>
          </Button>
          <Badge variant="dot" status="neutral" role="status">
            {status}
          </Badge>
          <Button
            variant="ghost"
            size="sm"
            leadingIcon={Refresh}
            aria-label="刷新"
            loading={loading}
            onClick={refresh}
          />
          <SystemMenu open={open} />
        </TopNavActions>
      </TopNav>
      <Dialog open={command} onOpenChange={setCommand}>
        <DialogContent className="p-0">
          <DialogTitle className="sr-only">搜索与切换标签</DialogTitle>
          <Command label="搜索与切换标签">
            <CommandInput
              autoFocus
              placeholder={
                extensions.length ? "搜索标签与插件入口…" : "搜索已打开的标签…"
              }
            />
            <CommandList>
              <CommandEmpty>没有匹配的标签</CommandEmpty>
              <CommandItem
                onSelect={() => {
                  activate("home");
                  setCommand(false);
                }}
              >
                Home
              </CommandItem>
              {tabs.map((tab) => (
                <CommandTab
                  key={tab.tabKey}
                  title={tab.title}
                  page={tab.ownerPage}
                  onSelect={() => {
                    activate(tab.tabKey);
                    setCommand(false);
                  }}
                />
              ))}
              {extensions
                .filter(
                  (entry) => !tabs.some((tab) => tab.routeHash === entry.href),
                )
                .map((entry) => (
                  <CommandTab
                    key={entry.id}
                    title={entry.label}
                    icon={entry.icon}
                    onSelect={() => {
                      open(entry.href);
                      setCommand(false);
                    }}
                  />
                ))}
            </CommandList>
          </Command>
        </DialogContent>
      </Dialog>
    </AppShellHeader>
  );
}

function CommandTab({
  title,
  page,
  icon,
  onSelect,
}: {
  title: string;
  page?: WorkspaceTabs["homePage"];
  icon?: IconName;
  onSelect: () => void;
}) {
  const item = page ? pageRegistry.resolve(page) : undefined;
  const Icon = useIcon(icon ?? item?.icon ?? "file-text");
  return (
    <CommandItem onSelect={onSelect}>
      <NavItemLeading>
        <Icon size={16} />
      </NavItemLeading>
      {title}
    </CommandItem>
  );
}
