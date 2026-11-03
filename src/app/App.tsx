import { pageRegistry, renderExtension } from "./builtin-plugins";
import type { PageContext, PageContribution } from "./plugin-registry";
import { RenderBoundary } from "./render-boundary";
import { AgentDirectoryProvider } from "@/shared/hooks/use-agent-directory";
import type { Page } from "@/app/types";
import { useSnapshot } from "@/app/use-snapshot";
import {
  Alert,
  AlertAction,
  AlertDescription,
  AlertTitle,
} from "@/components/ui/alert";
import {
  AppShell,
  AppShellMain,
  AppShellSidebar,
} from "@/components/ui/app-shell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  NavItem,
  NavItemContent,
  NavItemLabel,
  NavItemLeading,
  NavItemTrigger,
} from "@/components/ui/nav-item";
import { NavMenu } from "@/components/ui/nav-menu";
import {
  PageActions,
  PageBody,
  PageContent,
  PageContentHeader,
  PageLayout,
  PageTitle,
} from "@/components/ui/page-layout";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarProvider,
  SidebarTrigger,
  useSidebar,
} from "@/components/ui/sidebar";
import { IconProvider, useIcon } from "@/lib/icon-context";
import "@/styles/app.css";
import { Suspense, useEffect, useRef, useState, type RefObject } from "react";

const navigation = pageRegistry.navigation;
const sidebarWidth = "13.5rem";
const readRoute = () => pageRegistry.readRoute(window.location.hash);

function WorkspacePage({
  render,
  context,
  ownsBody,
  bodyRef,
}: {
  render: PageContribution["render"];
  context: PageContext;
  ownsBody: boolean | undefined;
  bodyRef: RefObject<HTMLDivElement | null>;
}) {
  const content = render(context);
  return ownsBody ? (
    content
  ) : (
    <PageBody ref={bodyRef} className="relative p-4 sm:p-6">
      {content}
    </PageBody>
  );
}

function WindowTitlebar({ title, status }: { title: string; status: string }) {
  const { open, isMobile } = useSidebar();
  return (
    <header
      aria-label="窗口工具栏"
      data-tauri-drag-region
      className="flex h-8 shrink-0 items-center pr-4"
    >
      <div
        data-tauri-drag-region
        className="flex h-full shrink-0 items-center pl-24"
        style={{ width: open && !isMobile ? sidebarWidth : undefined }}
      >
        <SidebarTrigger label="切换导航" variant="ghost" size="sm" />
      </div>
      <div
        data-tauri-drag-region
        className="flex min-w-0 flex-1 items-center self-stretch pl-6 pr-3"
      >
        <PageTitle
          data-tauri-drag-region
          className="truncate text-label font-medium"
        >
          {title}
        </PageTitle>
      </div>
      <Badge variant="dot" status="neutral" role="status">
        {status}
      </Badge>
    </header>
  );
}

function NavigationLink({
  item,
  active,
}: {
  item: (typeof navigation)[number];
  active: boolean;
}) {
  const Icon = useIcon(item.icon);
  const sidebar = useSidebar();
  return (
    <NavItem value={item.id} active={active}>
      <NavItemTrigger
        href={`#/${item.id}`}
        onClick={() => sidebar.closeMobile()}
      >
        <NavItemLeading>
          <Icon size={20} />
        </NavItemLeading>
        <NavItemContent>
          <NavItemLabel>{item.label}</NavItemLabel>
        </NavItemContent>
      </NavItemTrigger>
    </NavItem>
  );
}

function Workspace() {
  const pageBodyRef = useRef<HTMLDivElement>(null);
  const [route, setRoute] = useState(readRoute);
  const [pageRefresh, setPageRefresh] = useState(0);
  const { data, error, loading, refresh } = useSnapshot();
  const RefreshIcon = useIcon("rotate-ccw");
  useEffect(() => {
    const handle = () => setRoute(readRoute());
    window.addEventListener("hashchange", handle);
    return () => window.removeEventListener("hashchange", handle);
  }, []);
  useEffect(() => {
    if (
      route.extensionPlugin ||
      pageRegistry.resolve(route.page).resetScrollOnObject
    )
      pageBodyRef.current?.scrollTo({ top: 0 });
  }, [route.page, route.objectId, route.extensionPlugin, route.extensionView]);
  const current = navigation.find((item) => item.id === route.page)!;
  const navigate = (page: Page) => {
    window.location.hash = `/${page}`;
  };
  const status =
    loading && !data
      ? "正在连接本地服务"
      : data
        ? error
          ? "数据连接中断"
          : "本地服务已连接"
        : "本地服务未连接";
  const context: PageContext = {
    route,
    routeSearch: window.location.hash.split("?")[1] ?? "",
    data,
    refresh,
    navigate,
    pageRefresh,
  };
  return (
    <AgentDirectoryProvider products={data?.agents}>
      <SidebarProvider
        defaultOpen
        persistenceKey="local-agent.sidebar"
        keyboardShortcut="mod+b"
      >
        <div className="flex h-dvh flex-col overflow-hidden bg-surface-base">
          <WindowTitlebar title={current.label} status={status} />
          <AppShell className="min-h-0 flex-1 overflow-hidden">
            <AppShellSidebar>
              <Sidebar
                width={sidebarWidth}
                collapsible="offcanvas"
                ariaLabel="应用导航"
                className="h-full max-xl:pt-8"
              >
                <SidebarContent>
                  <NavMenu activeValue={route.page} aria-label="主导航">
                    {navigation
                      .filter((item) => !item.footer)
                      .map((item) => (
                        <NavigationLink
                          key={item.id}
                          item={item}
                          active={route.page === item.id}
                        />
                      ))}
                  </NavMenu>
                </SidebarContent>
                <SidebarFooter>
                  <NavMenu activeValue={route.page} aria-label="偏好设置">
                    <NavigationLink
                      item={navigation.find((item) => item.footer)!}
                      active={route.page === "settings"}
                    />
                  </NavMenu>
                </SidebarFooter>
              </Sidebar>
            </AppShellSidebar>
            <AppShellMain className="flex min-h-0 flex-col">
              <PageLayout size="full">
                {error && (
                  <div className="px-3 pb-3">
                    <Alert status="default">
                      <AlertTitle>
                        {data
                          ? "连接暂时中断，保留上次读取的配置"
                          : "本机数据尚未连接"}
                      </AlertTitle>
                      <AlertDescription>
                        <p>{error}</p>
                      </AlertDescription>
                      {route.page !== "settings" && (
                        <AlertAction>
                          <Button
                            variant="secondary"
                            size="sm"
                            onClick={() => navigate("settings")}
                          >
                            查看连接诊断
                          </Button>
                        </AlertAction>
                      )}
                    </Alert>
                  </div>
                )}
                <PageContent>
                  {(!data || error || current.refreshInHeader) && (
                    <PageContentHeader>
                      <PageActions>
                        <Button
                          variant="tertiary"
                          size="sm"
                          leadingIcon={RefreshIcon}
                          loading={loading}
                          onClick={() => {
                            setPageRefresh((n) => n + 1);
                            void refresh();
                          }}
                        >
                          刷新
                        </Button>
                      </PageActions>
                    </PageContentHeader>
                  )}
                  <RenderBoundary resetKey={window.location.hash}>
                    <Suspense
                      fallback={
                        <div
                          role="status"
                          className="p-4 text-body text-fg-muted"
                        >
                          正在加载页面…
                        </div>
                      }
                    >
                      <WorkspacePage
                        render={
                          route.extensionPlugin
                            ? renderExtension
                            : current.render
                        }
                        context={context}
                        ownsBody={current.ownsBody}
                        bodyRef={pageBodyRef}
                      />
                    </Suspense>
                  </RenderBoundary>
                </PageContent>
              </PageLayout>
            </AppShellMain>
          </AppShell>
        </div>
      </SidebarProvider>
    </AgentDirectoryProvider>
  );
}

export default function App() {
  return (
    <RenderBoundary scope="application" resetKey="application">
      <IconProvider>
        <Workspace />
      </IconProvider>
    </RenderBoundary>
  );
}
