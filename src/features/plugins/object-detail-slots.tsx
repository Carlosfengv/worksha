import { useEffect, useState, useSyncExternalStore } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogTrigger,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { Tabs, TabsList, TabItem, TabPanel } from "@/components/ui/tabs";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import { formatDateTime } from "@/shared/lib/date-time";
import {
  capabilityAccessEpoch,
  subscribeCapabilityAccess,
} from "@/shared/lib/capability-access";
import type {
  ObjectContributionStatus,
  ObjectSelector,
  ObjectSlotQuery,
  ObjectSlotStatus,
  ObjectSummary,
  SummaryField,
  SummaryMetric,
} from "../../../contracts/plugins/objects/types";
import { objectContributionsApi } from "./api";
import { objectContributionResource, pluginDirectoryResource } from "./queries";
import { SlotBoundary, truncateSlotTitle } from "./slot-presentation";
import { useSlotFocusVisibility } from "./use-slot-focus-visibility";

const labels: Record<SummaryField, string> = {
  "session-count": "已记录会话数",
  "model-count": "已保存启用模型数",
  configuration: "连接配置",
  "quota-window-count": "已记录额度窗口数",
  "input-tokens": "输入 Token",
  "output-tokens": "输出 Token",
  "total-tokens": "总 Token",
  "attempt-count": "已记录尝试次数",
  "duration-ms": "已记录耗时",
  "request-outcome": "请求结果",
};
const targetLabels = {
  agent: "Agent",
  provider: "Provider",
  account: "账户",
  project: "项目",
  session: "会话",
  request: "请求",
};
function metricText(metric: SummaryMetric) {
  const value = metric.value;
  if (!value)
    return {
      unread: "尚未读取",
      "not-recorded": "未记录",
      unavailable: "暂不可用",
      partial: "记录不完整",
    }[metric.missingReason ?? "unavailable"];
  switch (value.kind) {
    case "count":
    case "tokens":
      return BigInt(value.value).toLocaleString("zh-CN");
    case "milliseconds":
      return `${value.value.toLocaleString("zh-CN", { maximumFractionDigits: 3 })} ms`;
    case "configuration":
      return value.configured ? "已配置" : "未配置";
    case "outcome":
      return {
        succeeded: "成功",
        failed: "失败",
        cancelled: "已取消",
        "in-progress": "进行中",
        unknown: "未知",
      }[value.value];
  }
}
type Entry = { parent: ObjectContributionStatus; slot: ObjectSlotStatus };
function summaryScope(object: ObjectSelector) {
  const frozen = "origin" in object && object.origin.kind === "unified";
  if (object.target === "request")
    return `范围：${frozen ? "原冻结查询中的" : "已记录的"}单条${object.unit === "model-response" ? (frozen ? "模型响应" : "原生模型响应") : "网关逻辑请求"}。`;
  if (frozen) return "范围：原冻结查询中的当前对象。";
  if (object.target === "provider") return "范围：已保存的连接配置。";
  if (object.target === "account") return "范围：已有额度快照中的窗口记录。";
  if (object.target === "session") return "范围：当前会话的累计已记录数据。";
  return "范围：当前对象可读的全部已记录数据。";
}
function queryFor({ parent, slot }: Entry): ObjectSlotQuery {
  return {
    pluginId: parent.pluginId,
    digest: parent.digest,
    slotId: slot.declaration.id,
  };
}

function SummaryRead({
  entry,
  object,
}: {
  entry: Entry;
  object: ObjectSelector;
}) {
  const access = useSyncExternalStore(
    subscribeCapabilityAccess,
    capabilityAccessEpoch,
  );
  const backend = useBackendReadEpoch();
  const [retry, setRetry] = useState(0);
  const [state, setState] = useState<{
    identity: string;
    access: symbol | null;
    backend: symbol | null;
    retry: number;
    summary: ObjectSummary | null;
    error: string | null;
  }>({
    identity: "",
    access: null,
    backend: null,
    retry: -1,
    summary: null,
    error: null,
  });
  const identity = JSON.stringify([
    object,
    queryFor(entry),
    entry.slot.contextIdentity,
    entry.slot.declaration.fields,
  ]);
  useEffect(() => {
    const [selectedObject, slot, expectedContextIdentity, fields] = JSON.parse(
      identity,
    ) as [ObjectSelector, ObjectSlotQuery, string, SummaryField[]];
    let alive = true;
    let reading = false;
    let reference: string | null = null;
    let expired = false;
    let deadline: ReturnType<typeof setTimeout> | undefined;
    const release = (objectRef: string) =>
      void objectContributionsApi
        .perform({ kind: "release", query: { slot, objectRef } })
        .catch(() => {});
    const publish = (summary: ObjectSummary | null, error: string | null) =>
      setState({ identity, access, backend, retry, summary, error });
    publish(null, null);
    const read = async () => {
      if (!alive || reading || expired || !reference) return;
      reading = true;
      try {
        const reply = await objectContributionsApi.perform({
          kind: "read",
          query: { slot, objectRef: reference },
        });
        if (
          reply.kind !== "summary" ||
          reply.summary.target !== selectedObject.target ||
          JSON.stringify(reply.summary.metrics.map((m) => m.field)) !==
            JSON.stringify(fields)
        )
          throw new Error("摘要字段与当前对象不一致。");
        if (alive && !expired) publish(reply.summary, null);
      } catch (reason) {
        if (alive && !expired)
          publish(
            null,
            reason instanceof Error ? reason.message : String(reason),
          );
      } finally {
        reading = false;
      }
    };
    void (async () => {
      try {
        const started = performance.now();
        const reply = await objectContributionsApi.perform({
          kind: "issue",
          request: { slot, object: selectedObject, expectedContextIdentity },
        });
        if (reply.kind !== "reference") throw new Error("对象引用响应无效。");
        reference = reply.reference.objectRef;
        if (!alive) {
          release(reference);
          return;
        }
        const ttl = BigInt(reply.reference.expiresAfterMs);
        if (ttl === 0n || ttl > 60_000n) throw new Error("对象引用期限无效。");
        const remaining = Number(ttl) - (performance.now() - started);
        if (remaining <= 0) throw new Error("对象引用响应已过期，请重新读取。");
        deadline = setTimeout(() => {
          expired = true;
          if (alive) publish(null, "摘要引用已过期，请重新读取。");
          if (reference) release(reference);
        }, remaining);
        await read();
      } catch (reason) {
        if (reference) release(reference);
        expired = true;
        if (alive)
          publish(
            null,
            reason instanceof Error ? reason.message : String(reason),
          );
      }
    })();
    const poll = () => {
      if (document.visibilityState === "visible") void read();
    };
    const timer = setInterval(poll, 10_000);
    document.addEventListener("visibilitychange", poll);
    return () => {
      alive = false;
      clearTimeout(deadline);
      clearInterval(timer);
      document.removeEventListener("visibilitychange", poll);
      if (reference) release(reference);
    };
  }, [identity, access, backend, retry]);
  const current =
    state.identity === identity &&
    state.access === access &&
    state.backend === backend &&
    state.retry === retry
      ? state
      : { summary: null, error: null };
  return (
    <div className="space-y-3">
      <p className="text-label text-fg-muted">{summaryScope(object)}</p>
      {current.error ? (
        <p role="alert" className="break-words text-body text-fg-muted">
          {current.error}
        </p>
      ) : !current.summary ? (
        <p role="status">正在读取对象摘要…</p>
      ) : (
        <>
          <p className="text-label text-fg-muted">
            {current.summary.coverage === "complete"
              ? "当前记录范围完整"
              : current.summary.coverage === "partial"
                ? "部分记录"
                : "数据暂不可用"}{" "}
            ·{" "}
            {
              {
                current: "当前快照",
                stale: "较早快照",
                unread: "尚未读取",
                failed: "读取失败",
              }[current.summary.freshness]
            }
          </p>
          <p className="text-label text-fg-muted">
            快照时间：{formatDateTime(current.summary.capturedAt)}
          </p>
          <dl className="grid min-w-0 grid-cols-1 gap-3 sm:grid-cols-2">
            {current.summary.metrics.map((metric) => (
              <div key={metric.field} className="min-w-0">
                <dt className="text-label text-fg-muted">
                  {labels[metric.field]}
                </dt>
                <dd className="break-all text-body">{metricText(metric)}</dd>
              </div>
            ))}
          </dl>
        </>
      )}
      <Button variant="tertiary" onClick={() => setRetry((old) => old + 1)}>
        重新读取摘要
      </Button>
    </div>
  );
}

function ObjectSlot({
  entry,
  object,
}: {
  entry: Entry;
  object: ObjectSelector;
}) {
  const [confirmation, setConfirmation] = useState<{
    kind: "grant" | "revoke";
    context: string;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const slotRoot = useSlotFocusVisibility();
  const { parent, slot } = entry;
  const confirm =
    confirmation?.context === slot.contextIdentity ? confirmation.kind : null;
  const missing = parent.dependencies.filter(
    (d) => !d.available && d.views.includes(slot.declaration.viewId),
  );
  const available =
    parent.enabled &&
    parent.active &&
    !parent.errorCode &&
    !slot.errorCode &&
    !missing.length;
  const commit = async () => {
    if (!confirm || busy) return;
    setBusy(true);
    setError(null);
    try {
      await objectContributionsApi.perform({
        kind: confirm,
        request: {
          slot: queryFor(entry),
          expectedActivationEpoch: parent.activationEpoch,
          expectedGrantRevision: parent.grantRevision,
          expectedDescriptorIdentity: slot.descriptorIdentity,
          expectedContextIdentity: slot.contextIdentity,
        },
      });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      return;
    } finally {
      setBusy(false);
    }
    // Restore an enabled trigger when the native dialog returns focus. The
    // acknowledged action is complete; a stale follow-up still fails its CAS.
    setConfirmation(null);
    await objectContributionResource.invalidate();
  };
  return (
    <Dialog
      open={confirm !== null}
      onOpenChange={(open) => {
        if (!open && !busy) setConfirmation(null);
      }}
    >
      <div ref={slotRoot} className="space-y-3">
        {!available ? (
          <div role="status" className="space-y-2 text-body text-fg-muted">
            <p>
              {missing.length
                ? "可选依赖暂不可用，此摘要视图已降级；基础详情仍可使用。"
                : "此对象扩展暂不可用。"}
            </p>
            {missing.map((d) => (
              <p key={d.pluginId} className="break-all">
                {d.pluginId} · 要求 {d.version} ·{" "}
                {d.reason === "version-mismatch"
                  ? "版本不匹配"
                  : "尚未启用或不可用"}
              </p>
            ))}
            <a href="#/plugins" className="underline">
              管理插件
            </a>
          </div>
        ) : slot.authorized ? (
          <SummaryRead entry={entry} object={object} />
        ) : (
          <p role="status" className="text-body text-fg-muted">
            尚未授权此对象摘要。
          </p>
        )}
        {(available || slot.authorized) && (
          <DialogTrigger
            render={
              <Button
                variant="tertiary"
                disabled={busy}
                onClick={() => {
                  setError(null);
                  setConfirmation({
                    kind: slot.authorized ? "revoke" : "grant",
                    context: slot.contextIdentity,
                  });
                }}
              />
            }
          >
            {slot.authorized ? "撤销摘要授权" : "授权此摘要"}
          </DialogTrigger>
        )}
      </div>
      <DialogContent className="max-h-[calc(100dvh-4rem)] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>
            {confirm === "revoke" ? "撤销对象摘要授权" : "授权对象摘要"}
          </DialogTitle>
          <DialogDescription>
            {targetLabels[slot.declaration.target]}
            详情中的此插槽，可读取以下字段。授权适用于该类对象，停用或更换插件版本后须重新核对。
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-3 text-body">
          <p className="break-all">
            插件：{parent.pluginId}
            <br />
            版本摘要：{parent.digest.slice(0, 12)}
          </p>
          <p>
            字段：
            {slot.declaration.fields.map((field) => labels[field]).join("、")}
          </p>
          <p className="break-words">用途：{slot.declaration.purpose}</p>
          {error && (
            <p role="alert" className="break-words">
              {error}
            </p>
          )}
        </div>
        <DialogFooter>
          <Button
            variant="tertiary"
            disabled={busy}
            onClick={() => setConfirmation(null)}
            autoFocus
          >
            取消
          </Button>
          <Button disabled={busy} onClick={() => void commit()}>
            {busy
              ? "正在处理…"
              : confirm === "revoke"
                ? "确认撤销授权"
                : "确认授权摘要"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

const subscribeHash = (listener: () => void) => {
  window.addEventListener("hashchange", listener);
  return () => window.removeEventListener("hashchange", listener);
};
const hash = () => window.location.hash;
function selectTab(value: string | null) {
  const [path, search] = window.location.hash.split("?");
  const params = new URLSearchParams(search);
  if (value) params.set("objectSlot", value);
  else params.delete("objectSlot");
  window.location.hash = `${path}${params.size ? `?${params}` : ""}`;
}

export function ObjectContributionSlots({
  object,
}: {
  object: ObjectSelector;
}) {
  const directory = useSharedRead(pluginDirectoryResource);
  const installed =
    directory.data?.items.some((item) =>
      item.manifest.requires?.includes("host.object-detail-slots@1"),
    ) ?? false;
  const resource = useSharedRead(objectContributionResource, installed);
  const currentHash = useSyncExternalStore(subscribeHash, hash);
  const requested = new URLSearchParams(currentHash.split("?")[1]).get(
    "objectSlot",
  );
  const entries = (resource.data?.items ?? [])
    .filter((parent) => parent.enabled)
    .flatMap((parent) =>
      parent.slots
        .filter((slot) => slot.declaration.target === object.target)
        .map((slot) => ({ parent, slot })),
    );
  const token = (entry: Entry) =>
    `${object.target}:${entry.parent.pluginId}:${entry.slot.declaration.id}`;
  const selected = requested?.startsWith(`${object.target}:`)
    ? requested
    : null;
  if (resource.error)
    return (
      <section aria-label="对象摘要扩展状态" className="space-y-2">
        <p role="alert" className="break-words text-body text-fg-muted">
          对象扩展目录暂不可用：{resource.error}
        </p>
        <Button variant="tertiary" onClick={() => void resource.reload()}>
          重新读取扩展目录
        </Button>
      </section>
    );
  if (!entries.length && !selected) return null;
  const tabs = entries.filter((e) => e.slot.declaration.placement === "tab");
  const active = selected ?? (tabs[0] ? token(tabs[0]) : "");
  const render = (entry: Entry) => (
    <SlotBoundary
      key={JSON.stringify([
        object,
        entry.parent.digest,
        entry.slot.descriptorIdentity,
      ])}
    >
      <ObjectSlot entry={entry} object={object} />
    </SlotBoundary>
  );
  return (
    <section aria-label="对象摘要扩展" className="min-w-0 space-y-4">
      <h3 className="text-title font-medium">对象摘要扩展</h3>
      {selected && !entries.some((e) => token(e) === selected) && (
        <div role="status" className="space-y-2">
          <p>此对象摘要插槽已停用、移除或不适用于当前对象。</p>
          <Button variant="tertiary" onClick={() => selectTab(null)}>
            返回当前详情
          </Button>
        </div>
      )}
      {entries
        .filter((e) => e.slot.declaration.placement === "card")
        .map((entry) => (
          <article
            key={token(entry)}
            aria-label={entry.slot.declaration.title}
            className="min-w-0 space-y-3 rounded-xl border border-border p-4"
          >
            <h4 className="break-words text-title font-medium">
              {entry.slot.declaration.title}
            </h4>
            {render(entry)}
          </article>
        ))}
      {!!tabs.length && (
        <Tabs
          value={active}
          onValueChange={(value) => selectTab(String(value))}
          variant="underline"
          color="default"
        >
          <TabsList aria-label="对象摘要页签">
            {tabs.map((entry) => (
              <TabItem
                key={token(entry)}
                value={token(entry)}
                label={truncateSlotTitle(entry.slot.declaration.title)}
                aria-label={entry.slot.declaration.title}
                title={entry.slot.declaration.title}
              />
            ))}
          </TabsList>
          {tabs.map((entry) => (
            <TabPanel
              key={token(entry)}
              value={token(entry)}
              className="min-w-0 space-y-3"
            >
              {active === token(entry) && (
                <>
                  {truncateSlotTitle(entry.slot.declaration.title) !==
                    entry.slot.declaration.title && (
                    <h4 className="break-words text-title font-medium">
                      {entry.slot.declaration.title}
                    </h4>
                  )}
                  {render(entry)}
                </>
              )}
            </TabPanel>
          ))}
        </Tabs>
      )}
    </section>
  );
}
