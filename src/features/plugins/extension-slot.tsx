import { useCallback, useContext, useId, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogTitle,
} from "@/components/ui/dialog";
import { Tabs, TabsList, TabItem, TabPanel } from "@/components/ui/tabs";
import { usePollingResource } from "@/shared/hooks/use-polling-resource";
import type { ObjectSelector } from "../../../contracts/plugins/objects/types";
import type {
  EntityKind,
  PlatformAction,
  PlatformContext,
  PlatformContribution,
  SlotPoint,
} from "../../../contracts/plugins/platform/types";
import { platformPluginsApi } from "./api";
import { CollectionBlock } from "./blocks/collection-block";
import type { RecordValues } from "./blocks/record-form";
import { PluginDetailSlots } from "./detail-slots";
import { pluginViewHref } from "./navigation";
import { ObjectContributionSlots } from "./object-detail-slots";
import { ExtensionSlotContext } from "./platform-directory";
import { PlatformControls, SinkReadout } from "./platform-controls";
import { SlotBoundary, truncateSlotTitle } from "./slot-presentation";

const entityLabels: Record<EntityKind, string> = {
  session: "会话",
  project: "项目",
  agent: "Agent",
  provider: "供应商",
  account: "账户",
  request: "请求",
};

export function PlatformSlotContent({
  plugin,
  id,
  context,
  compact = false,
}: {
  plugin: PlatformContribution;
  id: string;
  context?: PlatformContext;
  compact?: boolean;
}) {
  const [cursors, setCursors] = useState<string[]>([]);
  const submitting = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [grant, setGrant] = useState<EntityKind>();
  const cursor = cursors.at(-1);
  const { pluginId, digest } = plugin;
  const serializedContext = JSON.stringify(context ?? null);
  const load = useCallback(
    () =>
      platformPluginsApi.query({
        pluginId,
        digest,
        slotOrViewId: id,
        context: JSON.parse(serializedContext),
        cursor,
      }),
    [pluginId, digest, id, serializedContext, cursor],
  );
  const resource = usePollingResource(
    JSON.stringify([
      digest,
      id,
      serializedContext,
      cursor,
      plugin.grantRevision,
    ]),
    load,
    true,
    8_000,
  );
  const view = resource.data?.blocks[0];
  const collection = view?.collection
    ? plugin.definition?.collections?.[view.collection]
    : undefined;
  const declaration = plugin.definition?.slots?.find((slot) => slot.id === id);
  const nav = plugin.definition?.slots?.find(
    (slot) => slot.point === "nav.home-sidebar",
  );
  const perform = async (operation: () => Promise<unknown>) => {
    if (submitting.current) return false;
    submitting.current = true;
    setBusy(true);
    setError(undefined);
    try {
      await operation();
      await resource.invalidate().catch(() => {
        setError("操作已完成，扩展数据刷新失败，请重新读取。");
      });
      return true;
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      return false;
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  const action = (
    actionId: PlatformAction,
    values: RecordValues,
    recordId?: string,
    expectedValues?: RecordValues,
  ) =>
    view && resource.data
      ? perform(() =>
          platformPluginsApi.invoke({
            storeId: resource.data!.actionStoreId,
            pluginId,
            digest,
            actionId,
            values,
            expectedValues,
            recordId,
            context,
            collection: view.collection,
            expectedEpoch: resource.data!.epoch,
            expectedGrantRevision: resource.data!.grantRevision,
          }),
        )
      : Promise.resolve(false);

  const grants =
    collection && view?.kind !== "stat-row"
      ? [
          ...new Set(
            Object.values(collection.fields).flatMap((field) =>
              field.kind.startsWith("ref:")
                ? [field.kind.slice(4) as EntityKind]
                : [],
            ),
          ),
        ].filter((target) => !plugin.entityGrants.includes(target))
      : [];

  if (resource.error)
    return (
      <Button
        size="sm"
        variant="tertiary"
        title={resource.error}
        onClick={() => void resource.reload()}
      >
        重试扩展
      </Button>
    );
  if (!resource.data || !view)
    return compact ? null : (
      <p role="status" className="text-body text-fg-muted">
        正在读取扩展…
      </p>
    );
  const binding =
    declaration?.filterByContext && context
      ? { field: declaration.filterByContext, value: context.objectId }
      : undefined;
  const next = resource.data.nextCursor;
  return (
    <div
      className={
        compact ? "inline-flex items-center gap-1" : "min-w-0 space-y-3"
      }
    >
      {error && (
        <p role="alert" className="text-label text-fg-muted">
          {error}
        </p>
      )}
      {!compact &&
        grants.map((target) => (
          <Button
            key={target}
            size="sm"
            variant="tertiary"
            onClick={() => setGrant(target)}
          >
            授权{entityLabels[target]}摘要
          </Button>
        ))}
      {collection ? (
        <CollectionBlock
          view={view}
          collection={collection}
          records={resource.data.records}
          action={action}
          compact={compact}
          binding={binding}
          next={next}
          loadMore={() => {
            if (next) setCursors((previous) => [...previous, next]);
          }}
          busy={busy}
          error={error}
        />
      ) : nav && compact ? (
        <Button asChild size="sm" variant="tertiary">
          <a data-workspace-pin href={pluginViewHref(pluginId, nav.viewId)}>
            {view.title}
          </a>
        </Button>
      ) : plugin.definition?.logSink ? (
        <section className="space-y-3">
          <h3 className="text-title font-medium">{view.title}</h3>
          {context ? (
            <SinkReadout plugin={{ ...plugin, sink: resource.data.sink }} />
          ) : (
            <PlatformControls
              plugin={{ ...plugin, sink: resource.data.sink }}
              sinkOnly
            />
          )}
        </section>
      ) : (
        <h3 className="text-title font-medium">{view.title}</h3>
      )}
      {!compact && cursors.length > 0 && (
        <Button
          size="sm"
          variant="tertiary"
          onClick={() => setCursors((previous) => previous.slice(0, -1))}
        >
          上一页
        </Button>
      )}
      {grant && (
        <Dialog
          open
          onOpenChange={(open) => {
            if (!open && !busy) setGrant(undefined);
          }}
        >
          <DialogContent>
            <DialogTitle>授权读取{entityLabels[grant]}摘要</DialogTitle>
            <DialogDescription>
              允许此插件显示关联对象的可用摘要。授权可在插件中心撤销。
            </DialogDescription>
            <DialogFooter>
              <Button
                variant="secondary"
                disabled={busy}
                onClick={() => setGrant(undefined)}
              >
                取消
              </Button>
              <Button
                loading={busy}
                onClick={async () => {
                  const granted = await perform(() =>
                    platformPluginsApi.manage({
                      pluginId,
                      digest,
                      expectedGrantRevision: plugin.grantRevision,
                      operation: { kind: "grant-entity", target: grant },
                    }),
                  );
                  if (granted) setGrant(undefined);
                }}
              >
                授权
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}
    </div>
  );
}

export function ExtensionSlot({
  points,
  context,
  object,
  legacyDetail = false,
}: {
  points: readonly SlotPoint[];
  context?: PlatformContext;
  object?: ObjectSelector;
  legacyDetail?: boolean;
}) {
  const { builtin, platform } = useContext(ExtensionSlotContext);
  const [selected, setSelected] = useState("");
  const region = useId();
  const entries = platform.flatMap((plugin) =>
    (plugin.definition?.slots ?? [])
      .filter((slot) => points.includes(slot.point))
      .map((slot) => ({ slot, plugin })),
  );
  const tabs = entries.filter(({ slot }) => slot.point.endsWith(".tab"));
  const detailTarget =
    legacyDetail &&
    context &&
    (context.target === "agent" ||
      context.target === "provider" ||
      context.target === "account")
      ? context.target
      : undefined;
  const keyOf = (entry: (typeof entries)[number]) =>
    `${entry.plugin.digest}:${entry.slot.id}`;
  const current = tabs.some((entry) => keyOf(entry) === selected)
    ? selected
    : tabs[0]
      ? keyOf(tabs[0])
      : "";
  const render = (entry: (typeof entries)[number]) => (
    <SlotBoundary key={`${keyOf(entry)}:${context?.objectId ?? ""}`}>
      <PlatformSlotContent
        plugin={entry.plugin}
        id={entry.slot.id}
        context={context}
        compact={entry.slot.point.endsWith(".actions")}
      />
    </SlotBoundary>
  );
  return (
    <>
      {detailTarget && context && (
        <PluginDetailSlots target={detailTarget} objectId={context.objectId} />
      )}
      {object && <ObjectContributionSlots object={object} />}
      {builtin
        .filter((slot) => points.includes(slot.point))
        .map(({ id, component: Component }) => (
          <SlotBoundary key={id}>
            <Component context={context} />
          </SlotBoundary>
        ))}
      {entries.filter(({ slot }) => !slot.point.endsWith(".tab")).map(render)}
      {tabs.length > 0 && (
        <Tabs
          id={region}
          value={current}
          onValueChange={(value) => setSelected(String(value))}
          variant="underline"
        >
          <TabsList aria-label="插件页签">
            {tabs.map((entry) => {
              const title =
                entry.plugin.definition?.views?.[entry.slot.viewId]?.title ??
                entry.slot.viewId;
              return (
                <TabItem
                  key={keyOf(entry)}
                  value={keyOf(entry)}
                  label={truncateSlotTitle(title)}
                  title={title}
                  aria-label={title}
                />
              );
            })}
          </TabsList>
          {tabs.map((entry) => (
            <TabPanel key={keyOf(entry)} value={keyOf(entry)} className="py-3">
              {current === keyOf(entry) && render(entry)}
            </TabPanel>
          ))}
        </Tabs>
      )}
    </>
  );
}
