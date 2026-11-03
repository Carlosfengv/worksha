import type { Provider } from "@/features/providers/types";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { pluginDirectoryResource } from "./queries";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { PluginViewContent } from "./view-content";
export function ExtensionsPage({
  pluginId,
  viewId,
  providers,
  refreshSnapshot,
}: {
  pluginId: string;
  viewId: string;
  providers: Provider[];
  refreshSnapshot: () => Promise<void>;
}) {
  const resource = useSharedRead(pluginDirectoryResource);
  const status = resource.data?.items.find(
    (item) => item.manifest.id === pluginId && item.enabled,
  );
  if (!status)
    return (
      <ConnectionEmpty
        title={resource.loading ? "正在读取扩展" : "插件不可用"}
        description={
          resource.error ?? "插件可能已停用、卸载或尚未启用；现有功能仍可使用。"
        }
        action="返回插件管理"
        onAction={() => {
          window.location.hash = "/plugins";
        }}
      />
    );
  return (
    <PluginViewContent
      key={`${status.digest}:${viewId}`}
      status={status}
      viewId={viewId}
      providers={providers}
      reloadDirectory={resource.invalidate}
      refreshSnapshot={refreshSnapshot}
      diagnostics={resource.data?.diagnostics}
      diagnosticsUnavailable={
        !!resource.error || !!resource.data?.diagnosticsError
      }
    />
  );
}
