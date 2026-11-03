import type { ContributionCatalog } from "../../../contracts/plugins/contributions/types";
import type { PluginStatus } from "./types";

export function PluginOptionalDependencies({
  status,
  catalog,
  error,
}: {
  status: PluginStatus;
  catalog?: ContributionCatalog | null;
  error?: string;
}) {
  const c = catalog?.items.find(
    (c) => c.pluginId === status.manifest.id && c.digest === status.digest,
  );
  if (
    !status.manifest.requires?.includes("host.optional-dependencies@1") &&
    !c?.slots.length
  )
    return null;
  return (
    <section
      aria-label="可选依赖与详情插槽"
      className="space-y-2 text-label text-fg-muted"
    >
      <h4 className="text-body font-medium">可选依赖与详情插槽</h4>
      {error || c?.errorCode ? (
        <p role="alert">声明暂不可用，现有管理操作仍可使用。</p>
      ) : !c ? (
        <p>当前服务未提供此声明的运行状态。</p>
      ) : (
        <>
          {c.dependencies.map((d) => (
            <p key={d.pluginId}>
              {d.pluginId} · 要求 {d.version} ·{" "}
              {d.available
                ? "依赖已启用"
                : d.reason === "version-mismatch"
                  ? "版本不匹配，相关视图已降级"
                  : "尚未启用或不可用，相关视图已降级"}{" "}
              · 影响视图 {d.views.join("、")}
            </p>
          ))}
          {!!c.slots.length && (
            <p>
              已声明 {c.slots.length}{" "}
              个详情插槽；只展示本插件已有信息，不继承当前对象的授权。
            </p>
          )}
        </>
      )}
    </section>
  );
}
