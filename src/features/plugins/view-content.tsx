import { useCallback, useMemo, useRef, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import type { Provider } from "@/features/providers/types";
import { usePollingResource } from "@/shared/hooks/use-polling-resource";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { pluginsApi } from "./api";
import { PluginControls } from "./controls";
import { PluginDiagnosticsPanel } from "./diagnostics-panel";
import type { PluginDiagnostics } from "./diagnostics";
import { object, parseContract } from "./contract";
import type {
  Conversation,
  PluginStatus,
  PluginView,
  SessionCandidate,
} from "./types";

type SessionRow = { sessionId: string; record: SessionCandidate };
const columns: ColumnDef<SessionRow>[] = [
  {
    id: "title",
    header: "会话",
    cell: ({ row }) => (
      <p className="max-w-80 break-words">
        {row.original.record.title || "未命名会话"}
      </p>
    ),
  },
  {
    id: "time",
    header: "原生时间",
    cell: ({ row }) => row.original.record.createdAt ?? "时间未知",
  },
  {
    id: "model",
    header: "模型",
    cell: ({ row }) => row.original.record.model ?? "未知",
  },
  {
    id: "input",
    header: "输入 Token",
    cell: ({ row }) => row.original.record.usage?.inputTokens ?? "未知",
  },
  {
    id: "output",
    header: "输出 Token",
    cell: ({ row }) => row.original.record.usage?.outputTokens ?? "未知",
  },
];
function SessionTable({
  data,
  loading,
  onRead,
}: {
  data: PluginView;
  loading: boolean;
  onRead?: (id: string) => void;
}) {
  const rows = useMemo(() => data.items ?? [], [data.items]);
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => row.sessionId,
    enableSorting: false,
    enableRowSelection: false,
    manualPagination: true,
    pageCount: -1,
  });
  return (
    <DataTable
      table={table}
      isLoading={loading}
      showPagination={false}
      aria-label="扩展会话列表"
      emptyMessage={
        data.confirmedEmpty
          ? data.status.errorCode
            ? "上次完整快照没有会话；当前读取状态未知"
            : "此来源已确认没有会话"
          : "尚未读取此来源"
      }
      onRowActivate={
        onRead ? (row) => onRead(row.original.sessionId) : undefined
      }
    />
  );
}
type ViewContentProps = {
  status: PluginStatus;
  viewId: string;
  providers: Provider[];
  reloadDirectory: () => void;
  refreshSnapshot: () => Promise<void>;
  diagnostics?: PluginDiagnostics | null;
  diagnosticsUnavailable: boolean;
  embedded?: boolean;
};

export function PluginViewContent(props: ViewContentProps) {
  if (props.status.manifest.requires?.includes("host.object-detail-slots@1"))
    return (
      <section aria-label="对象摘要扩展使用范围" className="space-y-4">
        <h1 className="text-heading font-medium">
          {props.status.manifest.displayName}
        </h1>
        <p role="status" className="text-body text-fg-muted">
          请从
          Agent、Provider、账户、项目、会话或请求详情中打开此扩展，并逐项确认摘要字段与用途。
        </p>
        <p className="text-label text-fg-muted">
          当前安装版本 {props.status.manifest.version}
          。打开此页面不读取对象摘要或刷新来源。
        </p>
        <PluginControls
          status={props.status}
          providers={props.providers}
          onChanged={props.reloadDirectory}
        />
      </section>
    );
  return <LegacyPluginViewContent {...props} />;
}
function LegacyPluginViewContent({
  status,
  viewId,
  providers,
  reloadDirectory,
  refreshSnapshot,
  diagnostics,
  diagnosticsUnavailable,
  embedded = false,
}: ViewContentProps) {
  const sourceReport = status.manifest.requires?.includes(
    "host.domain-services@1",
  );
  const [cursor, setCursor] = useState<string | null>(null);
  const loader = useCallback(
    () => pluginsApi.query({ pluginId: status.manifest.id, viewId, cursor }),
    [status.manifest.id, viewId, cursor],
  );
  const resource = usePollingResource(
    `plugin-view:${status.manifest.id}:${viewId}:${cursor}:${status.grantRevision}:${status.activationEpoch}`,
    loader,
  );
  const [busy, setBusy] = useState(false);
  const bodyGeneration = useRef(0);
  const [error, setError] = useState<string | null>(null);
  const [body, setBody] = useState<{
    conversation: Conversation;
    revision: string;
    epoch: string;
  } | null>(null);
  const snapshotChanged =
    !!resource.data &&
    (resource.data.status.digest !== status.digest ||
      resource.data.status.activationEpoch !== status.activationEpoch ||
      resource.data.status.grantRevision !== status.grantRevision);
  const data = snapshotChanged ? null : resource.data;
  const changed = () => {
    bodyGeneration.current++;
    setBody(null);
    setCursor(null);
    void resource.invalidate();
    reloadDirectory();
  };
  const run = async (operation: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setBody(null);
    try {
      await operation();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  const refresh = () =>
    run(async () => {
      await pluginsApi.action({
        pluginId: status.manifest.id,
        digest: status.digest,
        action: { kind: "refresh" },
      });
      changed();
    });
  const read = (sessionId: string) =>
    run(async () => {
      if (!data?.snapshot) return;
      const generation = bodyGeneration.current;
      const value = await pluginsApi.action({
        pluginId: status.manifest.id,
        digest: status.digest,
        action: { kind: "read-session", sessionId, snapshot: data.snapshot },
      });
      if (generation !== bodyGeneration.current) return;
      setBody({
        conversation: parseContract<Conversation>("conversation", value),
        revision: status.grantRevision,
        epoch: status.activationEpoch,
      });
    });
  const view = status.manifest.views?.find((view) => view.id === viewId);
  if (!view)
    return (
      <ConnectionEmpty
        title="插件视图不可用"
        description="该视图已被移除或未登记。"
        action="返回插件管理"
        onAction={() => {
          window.location.hash = "/plugins";
        }}
      />
    );
  const visibleBody =
    !snapshotChanged &&
    !resource.error &&
    body?.revision === status.grantRevision &&
    body.epoch === status.activationEpoch
      ? body.conversation
      : null;
  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          {embedded ? (
            <h4 className="text-title font-medium">{view.title}</h4>
          ) : (
            <h1 className="text-heading font-medium">{view.title}</h1>
          )}
          <p className="mt-1 text-body text-fg-muted">{view.description}</p>
          <p className="mt-2 text-label text-fg-subtle">
            用户上传 · {status.manifest.displayName} · {status.manifest.version}
          </p>
        </div>
        <Button variant="tertiary" asChild>
          <a
            href={
              embedded
                ? `#/extensions/${encodeURIComponent(status.manifest.id)}/${encodeURIComponent(viewId)}`
                : "#/plugins"
            }
          >
            {embedded ? "打开扩展" : "插件管理"}
          </a>
        </Button>
      </div>
      {!embedded && (
        <PluginControls
          key={status.digest}
          status={status}
          providers={providers}
          onChanged={changed}
        />
      )}
      {!embedded && (
        <PluginDiagnosticsPanel
          status={status}
          diagnostics={diagnostics}
          unavailable={diagnosticsUnavailable}
        />
      )}
      {snapshotChanged && (
        <div role="status" className="space-y-2 text-body text-fg-muted">
          <p>插件版本或授权已变化，请重新读取目录。</p>
          <Button variant="tertiary" onClick={reloadDirectory}>
            重新读取插件目录
          </Button>
        </div>
      )}
      {(error || resource.error || status.errorCode) && (
        <Alert status="danger">
          <AlertTitle>扩展当前不可用或已过期</AlertTitle>
          <AlertDescription>
            {error ??
              resource.error ??
              `插件状态：${status.runtimeStatus}。上次完整结果可能已经过期，请手动重试。`}
          </AlertDescription>
        </Alert>
      )}
      {!embedded &&
        (view.kind === "sessions" ||
          view.kind === "quota" ||
          view.kind === "report") && (
          <Button
            loading={busy}
            disabled={
              view.kind === "sessions"
                ? !status.sourceAuthorized
                : view.kind === "quota"
                  ? !status.connectionAuthorized
                  : !status.reportAuthorized
            }
            onClick={() => void refresh()}
          >
            手动刷新
            {view.kind === "sessions"
              ? "会话"
              : view.kind === "quota"
                ? "额度"
                : "报表"}
          </Button>
        )}
      {resource.loading && !data && (
        <p role="status" className="text-body text-fg-muted">
          正在读取扩展视图…
        </p>
      )}
      {data && !resource.error && view.kind === "report" && (
        <section
          className="space-y-3"
          aria-label={sourceReport ? "来源会话报表" : "网关摘要报表"}
        >
          <h2 className="text-title font-medium">
            {sourceReport ? "已授权来源摘要" : "最近 24 小时网关摘要"}
          </h2>
          {data.report?.sources?.length ? (
            <div className="space-y-3">
              {data.report.sources.map((source) => (
                <div key={source.sourceId} className="space-y-1">
                  <p className="text-body">
                    {source.sourceName || source.sourceId} · {source.sessions}{" "}
                    个会话 · {source.totalTokens ?? "未知"} Token
                  </p>
                  <p className="text-label text-fg-muted">
                    {source.coverage === "complete"
                      ? "范围已核查"
                      : "范围不完整"}{" "}
                    · 授权或来源变化后需重新读取
                    {data.reportExpired ? " · 上次快照已过期" : ""}
                  </p>
                </div>
              ))}
            </div>
          ) : data.report ? (
            <>
              <p className="text-body">
                逻辑请求 {data.report.requests} · 尝试 {data.report.attempts}
              </p>
              <p className="text-body">
                输入 {data.report.inputTokens ?? "未知"} · 输出{" "}
                {data.report.outputTokens ?? "未知"} · 总 Token{" "}
                {data.report.totalTokens ?? "未知"}
              </p>
              <p className="text-label text-fg-muted">
                {data.report.rangeStart} 至 {data.report.rangeEnd}
                {data.reportExpired ? " · 快照已过期" : ""}
                ；沿用现有网关统计口径，不与原生响应相加。
              </p>
            </>
          ) : (
            <p className="text-body text-fg-muted">
              {status.reportAuthorized
                ? "尚未读取，请手动刷新报表。"
                : "请先授权报表汇总。"}
            </p>
          )}
        </section>
      )}
      {data && !resource.error && view.kind === "sessions" && (
        <section className="space-y-4" aria-label="外部来源会话">
          {data.needsRefresh && (
            <p className="text-body text-fg-muted">
              {status.sourceAuthorized
                ? "已授权，尚未读取。请手动刷新会话。"
                : "请选择并授权 JSONL 文件。"}
            </p>
          )}
          {data.summary && (
            <p className="text-label text-fg-muted">
              完整快照 {data.total} 个会话 · 已知用量合计{" "}
              {data.summary.totalTokens} Token。只计本扩展来源；不与现有原生 /
              网关记录相加。
            </p>
          )}
          <SessionTable
            data={data}
            loading={busy}
            onRead={embedded ? undefined : (id) => void read(id)}
          />
          <div className="flex flex-wrap gap-2">
            <Button
              variant="tertiary"
              disabled={!cursor || busy}
              onClick={() => setCursor(null)}
            >
              返回第一页
            </Button>
            <Button
              variant="tertiary"
              disabled={!data.nextCursor || busy}
              onClick={() => setCursor(data.nextCursor ?? null)}
            >
              下一页
            </Button>
            <span className="self-center text-label text-fg-muted">
              本页 {data.items?.length ?? 0} 条 · 完整快照{" "}
              {data.total ?? "未知"} 条
            </span>
          </div>
          {visibleBody && (
            <article className="space-y-3" aria-label="扩展会话正文">
              <h2 className="text-title font-medium">只读会话正文</h2>
              {visibleBody.messages.map((message, index) => (
                <div
                  key={index}
                  className="rounded-xl border border-border p-4"
                >
                  <p className="text-label text-fg-muted">{message.role}</p>
                  <p className="mt-2 whitespace-pre-wrap break-words text-body">
                    {message.text}
                  </p>
                </div>
              ))}
            </article>
          )}
        </section>
      )}
      {data && !resource.error && view.kind === "quota" && (
        <section className="space-y-3" aria-label="扩展额度">
          <h2 className="text-title font-medium">额度快照</h2>
          {data.quota ? (
            <>
              <p className="text-heading">
                {data.quota.remaining} {data.quota.unit}
              </p>
              <p className="text-label text-fg-muted">
                上限：{data.quota.limit ?? "未知"} · 读取于{" "}
                {data.quota.observedAt}
              </p>
              <p className="text-label text-fg-muted">
                {data.quotaExpired
                  ? "快照已过期"
                  : data.quota.expiresAt
                    ? `有效期至 ${data.quota.expiresAt}`
                    : "有效期未知"}
                ；不代表已验证模型调用权限。
              </p>
            </>
          ) : (
            <p className="text-body text-fg-muted">
              {status.connectionAuthorized
                ? "尚未采集，请手动刷新额度。"
                : "请先授权绑定连接。"}
            </p>
          )}
        </section>
      )}
      {view.kind === "providers" && (
        <section className="space-y-4" aria-label="外部供应商预设">
          {status.manifest.providers?.map((preset) => (
            <article
              className="space-y-2 rounded-xl border border-border p-4"
              key={preset.id}
            >
              <h2 className="text-title font-medium">{preset.name}</h2>
              <p className="break-all text-label text-fg-muted">
                {preset.endpoint} · {preset.protocol}
              </p>
              {!embedded && (
                <Button
                  variant="tertiary"
                  disabled={busy}
                  onClick={() =>
                    void run(async () => {
                      const result = await pluginsApi.action({
                        pluginId: status.manifest.id,
                        digest: status.digest,
                        action: { kind: "create-provider", preset: preset.id },
                      });
                      if (
                        !object(result) ||
                        typeof result.providerId !== "string" ||
                        !/^custom-[\da-f-]{36}$/.test(result.providerId)
                      )
                        throw new Error("连接结果格式无效。");
                      await refreshSnapshot();
                      window.location.hash = `/providers/${encodeURIComponent(result.providerId)}`;
                    })
                  }
                >
                  创建本机连接
                </Button>
              )}
            </article>
          ))}
        </section>
      )}
      {embedded && (
        <p className="text-label text-fg-muted">
          仅展示此插件已有的数据；不会自动读取当前对象、执行采集或继承其授权。需要授权或刷新时，请打开扩展。
        </p>
      )}
      {view.kind === "settings" && (
        <p className="text-label text-fg-muted">
          {embedded
            ? "此扩展的设置和授权在独立扩展页管理。"
            : "上方设置由宿主保存，只允许包中列出的选项；插件不能写入任意文件或持久化会话正文。"}
        </p>
      )}
    </div>
  );
}
