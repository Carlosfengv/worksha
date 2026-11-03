import { Suspense, lazy, useCallback, useSyncExternalStore } from "react";
import { Button } from "@/components/ui/button";
import { Tabs, TabsList, TabItem, TabPanel } from "@/components/ui/tabs";
import { usePollingResource } from "@/shared/hooks/use-polling-resource";
import type {
  DetailTarget,
  DetailSlot,
  ContributionStatus,
} from "../../../contracts/plugins/contributions/types";
import { pluginsApi } from "./api";
import type { PluginStatus } from "./types";
import { useSlotFocusVisibility } from "./use-slot-focus-visibility";
import {
  SlotBoundary,
  truncateSlotTitle as tabLabel,
} from "./slot-presentation";
const PluginViewContent = lazy(() =>
  import("./view-content").then((module) => ({
    default: module.PluginViewContent,
  })),
);

const subscribe = (callback: () => void) => {
  window.addEventListener("hashchange", callback);
  return () => window.removeEventListener("hashchange", callback);
};
const currentHash = () => window.location.hash;
const noRefresh = async () => {};
const emptyProviders: never[] = [];
type Entry = {
  slot: DetailSlot;
  status: PluginStatus;
  contribution: ContributionStatus;
};
const token = (entry: Entry) =>
  `${entry.slot.target}:${entry.status.manifest.id}:${entry.slot.id}`;
function setSelected(next: string | null) {
  const [path, search] = window.location.hash.split("?");
  const params = new URLSearchParams(search);
  if (next) params.set("pluginSlot", next);
  else params.delete("pluginSlot");
  window.location.hash = `${path}${params.size ? `?${params}` : ""}`;
}
function SlotView({ entry, reload }: { entry: Entry; reload: () => void }) {
  const { slot, status, contribution } = entry;
  const missing = contribution.dependencies.filter(
    (d) => !d.available && d.views.includes(slot.viewId),
  );
  if (
    !status.enabled ||
    status.runtimeStatus !== "active" ||
    contribution.errorCode ||
    missing.length
  )
    return (
      <div role="status" className="space-y-2 text-body text-fg-muted">
        <p>
          {missing.length
            ? "可选依赖暂不可用，此扩展视图已降级；插件基础能力仍可使用。"
            : "此插件当前不可用，现有详情仍可使用。"}
        </p>
        {missing.map((d) => (
          <p key={d.pluginId} className="break-all">
            {d.pluginId} · 要求 {d.version} ·{" "}
            {d.reason === "version-mismatch"
              ? "版本不匹配"
              : "尚未启用或不可用"}
          </p>
        ))}
        <a className="underline" href="#/plugins">
          管理插件
        </a>
      </div>
    );
  return (
    <SlotBoundary
      key={`${status.digest}:${status.activationEpoch}:${status.grantRevision}`}
    >
      <Suspense fallback={<p role="status">正在加载扩展视图…</p>}>
        <PluginViewContent
          status={status}
          viewId={slot.viewId}
          providers={emptyProviders}
          reloadDirectory={reload}
          refreshSnapshot={noRefresh}
          diagnosticsUnavailable={false}
          embedded
        />
      </Suspense>
    </SlotBoundary>
  );
}

/** Host-owned read-only slots. Context IDs select declarations; no object data goes to a guest. */
export function PluginDetailSlots({
  target,
  objectId,
}: {
  target: DetailTarget;
  objectId: string;
}) {
  const focusRoot = useSlotFocusVisibility();
  const hash = useSyncExternalStore(subscribe, currentHash, currentHash);
  const loader = useCallback(() => pluginsApi.list(), []);
  const resource = usePollingResource(
    `plugin-details:${target}:${objectId}`,
    loader,
  );
  const requested = new URLSearchParams(hash.split("?")[1]).get("pluginSlot");
  const selected = requested?.startsWith(`${target}:`) ? requested : null;
  const entries: Entry[] = [];
  const catalog = resource.data?.contributions;
  for (const c of catalog?.items ?? []) {
    const status = resource.data?.items.find(
      (s) => s.manifest.id === c.pluginId && s.digest === c.digest && s.enabled,
    );
    if (!status) continue;
    for (const slot of c.slots)
      if (
        slot.target === target &&
        (slot.objectIds.includes("*") || slot.objectIds.includes(objectId))
      )
        entries.push({ slot, status, contribution: c });
  }
  const error = resource.error ?? resource.data?.contributionsError;
  if (error)
    return (
      <section
        ref={focusRoot}
        aria-label="详情扩展状态"
        className="space-y-2 text-body text-fg-muted"
      >
        <p role="alert">扩展信息暂不可用：{error}</p>
        <Button variant="tertiary" onClick={resource.reload}>
          重新读取扩展
        </Button>
      </section>
    );
  if (resource.loading && !resource.data)
    return (
      <p role="status" className="text-label text-fg-muted">
        正在读取扩展信息…
      </p>
    );
  const missing = selected && !entries.some((e) => token(e) === selected);
  if (!entries.length && !selected) return null;
  const tabs = entries.filter((e) => e.slot.placement === "tab");
  const active = selected ?? (tabs[0] ? token(tabs[0]) : "");
  return (
    <section
      ref={focusRoot}
      aria-label="详情扩展信息"
      className="min-w-0 space-y-4"
    >
      <h3 className="text-title font-medium">扩展信息</h3>
      {missing && (
        <div role="status" className="space-y-2 text-body text-fg-muted">
          <p>插件详情插槽不可用：可能已停用、移除或不适用于当前对象。</p>
          <Button variant="tertiary" onClick={() => setSelected(null)}>
            返回当前详情
          </Button>
        </div>
      )}
      {entries
        .filter((e) => e.slot.placement === "card")
        .map((entry) => (
          <article
            key={`${objectId}:${entry.status.digest}:${entry.slot.id}`}
            className="space-y-3 rounded-xl border border-border p-4"
            aria-label={entry.slot.title}
          >
            <h4 className="break-words text-title font-medium">
              {entry.slot.title}
            </h4>
            <SlotView entry={entry} reload={resource.invalidate} />
          </article>
        ))}
      {!!tabs.length && (
        <Tabs
          value={active}
          onValueChange={(next) => setSelected(String(next))}
          variant="underline"
          color="default"
        >
          <TabsList aria-label="详情扩展页签">
            {tabs.map((entry) => (
              <TabItem
                key={token(entry)}
                value={token(entry)}
                label={tabLabel(entry.slot.title)}
                aria-label={entry.slot.title}
                title={entry.slot.title}
              />
            ))}
          </TabsList>
          {tabs.map((entry) => (
            <TabPanel
              key={token(entry)}
              value={token(entry)}
              className="min-w-0 space-y-3"
            >
              {active === token(entry) &&
                tabLabel(entry.slot.title) !== entry.slot.title && (
                  <h4 className="break-words text-title font-medium">
                    {entry.slot.title}
                  </h4>
                )}
              {active === token(entry) && (
                <SlotView
                  key={`${objectId}:${entry.status.digest}:${token(entry)}`}
                  entry={entry}
                  reload={resource.invalidate}
                />
              )}
            </TabPanel>
          ))}
        </Tabs>
      )}
    </section>
  );
}
