import { Button } from "@/components/ui/button";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { pluginDirectoryResource } from "./queries";
import { domainDirectoryResource } from "@/features/domain/queries";
import { pluginViewHref } from "./navigation";

export function PluginEntries({ kind }: { kind: "agents" | "providers" }) {
  const resource = useSharedRead(pluginDirectoryResource);
  const unified = useSharedRead(domainDirectoryResource, kind === "agents");
  const items = resource.data?.items.filter(
    (s) =>
      s.enabled &&
      (kind === "agents" ? s.manifest.agent : s.manifest.providers?.length),
  );
  if (!items?.length && !resource.error) return null;
  return (
    <section
      className="space-y-3"
      aria-label={kind === "agents" ? "扩展 Agent" : "扩展供应商预设"}
    >
      <h2 className="text-title font-medium">
        {kind === "agents" ? "扩展 Agent" : "扩展供应商预设"}
      </h2>
      {resource.error && (
        <p className="text-label text-fg-muted">
          扩展目录暂不可用：{resource.error}
        </p>
      )}
      {kind === "agents" &&
        (unified.error || unified.data?.externalProblem) && (
          <p className="text-label text-fg-muted">
            统一来源目录暂不可用；原扩展视图仍可独立核查。
          </p>
        )}
      {items?.map((status) => {
        const view = status.manifest.views?.find(
          (view) =>
            view.kind === (kind === "agents" ? "sessions" : "providers"),
        );
        const unifiedSource = unified.data?.catalog.sources.find(
          (s) => s.id === status.manifest.id + ":source",
        );
        return (
          <div
            className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-border p-4"
            key={status.digest}
          >
            <div>
              <p>
                {kind === "agents"
                  ? status.manifest.agent?.displayName
                  : status.manifest.displayName}
              </p>
              <p className="text-label text-fg-muted">
                {kind === "agents"
                  ? status.sourceAuthorized
                    ? "来源已授权；读取状态见扩展视图"
                    : "来源待授权"
                  : "复制连接后仍需配置与验证"}
              </p>
              {kind === "agents" && unifiedSource && (
                <p className="text-label text-fg-muted">
                  {unifiedSource.status.authorization === "authorized"
                    ? "已加入统一展示"
                    : "统一展示待授权"}{" "}
                  ·{" "}
                  {unifiedSource.status.freshness === "current"
                    ? "元数据已读取"
                    : unifiedSource.status.freshness === "stale"
                      ? "元数据已过期"
                      : "尚需核查读取状态"}
                </p>
              )}
            </div>
            <div className="flex flex-wrap gap-2">
              {kind === "agents" &&
                status.manifest.capabilities.includes("session.source@1") && (
                  <Button variant="tertiary" asChild>
                    <a
                      href={`#/sessions?scope=unified&source=${encodeURIComponent(status.manifest.id + ":source")}`}
                    >
                      查看统一会话
                    </a>
                  </Button>
                )}
              {view && (
                <Button variant="tertiary" asChild>
                  <a href={pluginViewHref(status.manifest.id, view.id)}>
                    查看扩展
                  </a>
                </Button>
              )}
            </div>
          </div>
        );
      })}
    </section>
  );
}
