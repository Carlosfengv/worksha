import { useEffect, useRef } from "react";
import type { Provider } from "@/features/providers/types";
import type { ReadSnapshot } from "@/shared/lib/read-resource";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { PluginViewContent } from "./view-content";
import { resolvePluginView } from "./navigation";
import type { PluginList } from "./types";

const unavailableDescriptions = {
  disabled: "插件已停用，可在插件中心查看状态。",
  uninstalled: "插件已卸载或尚未安装。",
  unavailable: "插件运行状态或签名页面声明暂不可核查。",
  undeclared: "此版本未声明该页面。",
  dependency: "此页面依赖的插件或能力暂不可用。",
};
export function ExtensionsPage({
  pluginId,
  viewId,
  providers,
  refreshSnapshot,
  directory,
  reloadDirectory,
}: {
  pluginId: string;
  viewId: string;
  providers: Provider[];
  refreshSnapshot: () => Promise<void>;
  directory: ReadSnapshot<PluginList>;
  reloadDirectory: () => void;
}) {
  const page = resolvePluginView(directory.data ?? undefined, pluginId, viewId);
  const { loading, error } = directory;
  const unavailable = !loading && page.reason !== "ready";
  const fallback = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (unavailable) fallback.current?.focus();
  }, [unavailable, pluginId, viewId]);
  if (page.reason !== "ready")
    return (
      <div ref={fallback} tabIndex={-1} role="region" aria-label="插件页面状态">
        <ConnectionEmpty
          title={loading ? "正在读取扩展" : "插件不可用"}
          description={
            error ??
            (loading ? undefined : unavailableDescriptions[page.reason])
          }
          action="返回插件管理"
          onAction={() => {
            window.location.hash = "/plugins";
          }}
        />
      </div>
    );
  const { status } = page;
  return (
    <PluginViewContent
      key={`${status.digest}:${viewId}`}
      status={status}
      viewId={viewId}
      providers={providers}
      reloadDirectory={reloadDirectory}
      refreshSnapshot={refreshSnapshot}
      diagnostics={directory.data?.diagnostics}
      diagnosticsUnavailable={!!directory.data?.diagnosticsError}
    />
  );
}
