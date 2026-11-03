import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Button } from "@/components/ui/button";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { Field, FieldLabel } from "@/components/ui/field";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import { dataTablePagination } from "@/shared/ui/data-table-labels";
import { modelServicesResource, routingPoliciesResource } from "../queries";
import { routingPolicyApi } from "../api";
import { RoutingPolicyRecovery } from "./routing-policy-recovery";
import { serviceDraft, type ModelService } from "../types";
import type {
  Action,
  FailureMode,
  PackageStatus,
  PublishStatus,
  Review,
  StateAction,
} from "../../../../contracts/plugins/routing/types";

// Only match the host's normalized unknown list. Requests retain the exact
// original service spelling so distinct historical scopes are never merged.
function unknownScopeKey(id: string): string {
  const value = id.startsWith("urn:uuid:")
    ? id.slice(9)
    : id.startsWith("{") && id.endsWith("}")
      ? id.slice(1, -1)
      : id;
  return /^(?:[0-9a-f]{32}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/i.test(
    value,
  )
    ? value.replaceAll("-", "").toLowerCase()
    : id;
}

const labels: Record<StateAction, string> = {
  enable: "启用此版本",
  disable: "停用",
  "revoke-grants": "撤销策略授权",
  "revoke-publisher": "撤销发布者信任",
  remove: "卸载此版本",
  rollback: "回退上一版本",
};
const columns: ColumnDef<PackageStatus>[] = [
  {
    id: "name",
    header: "策略插件",
    cell: ({ row }) => (
      <div className="max-w-64 break-words">
        <p>{row.original.manifest.displayName}</p>
        <p className="text-label text-fg-muted">
          {row.original.manifest.version} · {row.original.manifest.id}
        </p>
      </div>
    ),
  },
  {
    id: "state",
    header: "运行授权",
    cell: ({ row }) => (
      <span>
        {row.original.active
          ? "已启用"
          : row.original.enabled
            ? "待重新启用"
            : "已停用"}{" "}
        · {row.original.publisherTrusted ? "发布者已信任" : "发布者未信任"}
      </span>
    ),
  },
];
type PublicationRow = PublishStatus & {
  disabled: boolean;
  query: () => void;
  cancel: () => void;
  resume: () => void;
};
const publicationColumns: ColumnDef<PublicationRow>[] = [
  {
    id: "operation",
    header: "策略发布",
    cell: ({ row }) => (
      <div className="max-w-64 break-all text-label">
        {row.original.operationId}
        <p>
          {(
            {
              prepared: "尚未开始提交",
              committing: "提交中，待核查",
              unknown: "结果未确认",
              committed: "发布记录已确认",
              cancelled: "已取消",
            } as Record<string, string>
          )[row.original.state] ?? "状态未知"}
        </p>
      </div>
    ),
  },
  {
    id: "actions",
    header: "结果核查",
    cell: ({ row }) => (
      <div className="flex flex-wrap gap-2">
        <Button
          size="sm"
          variant="tertiary"
          disabled={row.original.disabled}
          onClick={row.original.query}
        >
          核查原结果
        </Button>
        {["prepared", "committing", "unknown"].includes(row.original.state) && (
          <Button
            size="sm"
            variant="tertiary"
            disabled={row.original.disabled}
            onClick={row.original.resume}
          >
            继续原发布
          </Button>
        )}
        {row.original.state === "prepared" && (
          <Button
            size="sm"
            variant="tertiary"
            disabled={row.original.disabled}
            onClick={row.original.cancel}
          >
            取消未提交发布
          </Button>
        )}
      </div>
    ),
  },
];
export function ExternalRoutingPolicy({
  storeId,
  services,
}: {
  storeId: string;
  services: ModelService[];
}) {
  const resource = useSharedQuery(routingPoliciesResource, storeId);
  const [busy, setBusy] = useState(false);
  const gate = useRef(false);
  const lifetime = useRef(0);
  useEffect(() => {
    lifetime.current += 1;
    return () => {
      lifetime.current += 1;
    };
  }, []);
  const [error, setError] = useState<string | null>(null);
  const [review, setReview] = useState<Review | null>(null);
  const [resumePublication, setResumePublication] =
    useState<PublishStatus | null>(null);
  const [digest, setDigest] = useState<string | null>(null);
  const [action, setAction] = useState<StateAction>("enable");
  const [confirmation, setConfirmation] = useState<{
    item: PackageStatus;
    action: StateAction;
  } | null>(null);
  const [serviceId, setServiceId] = useState("");
  const [failure, setFailure] = useState<FailureMode>("reject");
  const data = resource.error ? null : resource.data;
  const selected = services.find((s) => s.id === serviceId);
  const currentPackage = data?.packages.find((p) => p.digest === digest);
  const scope = data?.scopes.find((s) => s.serviceId === serviceId);
  const binding = data?.bindings.find((b) => b.serviceId === serviceId);
  const locked = busy || !!resource.error || !data;
  const resumeRecord = resumePublication
    ? data?.publishes.find(
        (p) =>
          p.operationId === resumePublication.operationId &&
          p.storeId === resumePublication.storeId &&
          p.serviceId === resumePublication.serviceId,
      )
    : undefined;
  const registryUnknown =
    !!selected &&
    (data?.unavailableServiceIds ?? []).some(
      (id) => unknownScopeKey(id) === unknownScopeKey(selected.id),
    );
  const invalidate = resource.invalidate;
  const run = useCallback(
    async (work: (isCurrent: () => boolean) => Promise<void>) => {
      if (gate.current) return;
      gate.current = true;
      const generation = lifetime.current;
      const isCurrent = () => lifetime.current === generation;
      setBusy(true);
      setError(null);
      try {
        await work(isCurrent);
      } catch (reason) {
        if (isCurrent())
          setError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        if (isCurrent()) {
          await Promise.all([invalidate(), modelServicesResource.invalidate()]);
          if (isCurrent()) setBusy(false);
        }
        gate.current = false;
      }
    },
    [invalidate],
  );
  const mutate = async (input: Action) => {
    await routingPolicyApi.perform(input);
  };
  const items = useMemo(() => data?.packages ?? [], [data]);
  const pagination = useTablePagination(items.length, 5);
  const { table } = useDataTable({
    data: items,
    columns,
    getRowId: (p) => p.digest,
    enableSorting: false,
    enableRowSelection: false,
    enableHiding: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
    autoResetPageIndex: false,
  });
  const publications = useMemo<PublicationRow[]>(
    () =>
      data?.publishes.map((p) => ({
        ...p,
        disabled: locked,
        resume: () => {
          setError(null);
          setResumePublication(p);
        },
        query: () =>
          void run(() =>
            mutate({
              kind: "query-publish",
              storeId,
              operationId: p.operationId,
            }),
          ),
        cancel: () =>
          void run(() =>
            mutate({
              kind: "cancel-publish",
              storeId,
              operationId: p.operationId,
            }),
          ),
      })) ?? [],
    [data, locked, storeId, run],
  ); // Actions use immutable operation identities.
  const publicationPagination = useTablePagination(publications.length, 5);
  const publicationTable = useDataTable({
    data: publications,
    columns: publicationColumns,
    getRowId: (p) => p.operationId,
    enableSorting: false,
    enableRowSelection: false,
    enableHiding: false,
    state: { pagination: publicationPagination.value },
    onPaginationChange: publicationPagination.onChange,
    autoResetPageIndex: false,
  }).table;
  const bind = async (publish: boolean) => {
    if (!selected || !currentPackage || !data)
      throw Error("请先读取并选择服务与已启用策略。");
    const common = {
      package: {
        pluginId: currentPackage.manifest.id,
        digest: currentPackage.digest,
      },
      failureMode: failure,
      expectedBindingRevision: scope?.revision ?? null,
    };
    if (publish) {
      const operationId = crypto.randomUUID();
      setError(
        "发布编号：" + operationId + "。若结果不明，请在下方核查原结果。",
      );
      await mutate({
        kind: "publish",
        request: {
          storeId,
          operationId,
          action: { kind: "save", draft: serviceDraft(selected) },
        },
        ...common,
      });
    } else
      await mutate({
        kind: "bind",
        storeId,
        serviceId: selected.id,
        serviceVersionId: selected.versionId,
        ...common,
      });
  };
  return (
    <section className="space-y-4" aria-label="外部路由策略">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="space-y-2">
          <h3 className="text-heading-sm font-semibold">外部路由策略</h3>
          <p className="max-w-2xl text-body text-fg-muted">
            插件只对许可候选排序。服务能力、冷却、限流和发送仍由本机管理；更新插件后需重新确认服务版本绑定。
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="tertiary"
            loading={resource.loading}
            disabled={busy}
            onClick={resource.reload}
          >
            重新读取策略
          </Button>
          <Button
            disabled={busy}
            onClick={() =>
              void run(async (isCurrent) => {
                const result = await routingPolicyApi.perform({
                  kind: "inspect",
                  storeId,
                  path: "",
                });
                if (isCurrent() && result?.kind === "review")
                  setReview(result.review);
              })
            }
          >
            选择策略包
          </Button>
        </div>
      </div>
      {(error || resource.error) && (
        <p role="alert" className="break-words text-body text-fg-danger">
          {error ?? resource.error}
        </p>
      )}
      <DataTable
        table={table}
        aria-label="纯路由策略插件"
        isLoading={resource.loading && !resource.data}
        activeRowId={digest}
        onRowActivate={(row) => {
          if (!busy) setDigest(row.original.digest);
        }}
        paginationProps={{ labels: dataTablePagination }}
        emptyMessage={
          resource.error
            ? "策略目录暂不可读取"
            : "尚未安装外部路由策略。现有服务继续使用原有策略。"
        }
      />
      {currentPackage && (
        <div className="flex flex-wrap items-end gap-3">
          <Field>
            <FieldLabel>所选插件操作</FieldLabel>
            <Select
              value={action}
              disabled={locked}
              onValueChange={(value) => {
                if (value && value in labels) setAction(value as StateAction);
              }}
            >
              <SelectTrigger aria-label="策略插件操作" />
              <SelectContent>
                {(Object.keys(labels) as StateAction[]).map((a) => (
                  <SelectItem key={a} value={a}>
                    {labels[a]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>
          <Button
            variant="tertiary"
            disabled={locked}
            onClick={() => setConfirmation({ item: currentPackage, action })}
          >
            {labels[action]}
          </Button>
        </div>
      )}
      <div className="grid gap-4 md:grid-cols-2">
        <Field>
          <FieldLabel>模型服务</FieldLabel>
          <Combobox<string>
            items={services.map((s) => s.id)}
            value={serviceId || null}
            onValueChange={(id) => {
              setServiceId(id ?? "");
              setFailure(
                data?.bindings.find((b) => b.serviceId === id)?.failureMode ??
                  "reject",
              );
            }}
            itemToStringLabel={(id) =>
              services.find((s) => s.id === id)?.name ?? id
            }
            disabled={locked}
          >
            <ComboboxInput
              aria-label="选择策略所属服务"
              placeholder="搜索模型服务"
            />
            <ComboboxContent>
              <ComboboxEmpty>暂无模型服务</ComboboxEmpty>
              <ComboboxList>
                {(id) => (
                  <ComboboxItem key={id} value={id}>
                    {services.find((s) => s.id === id)?.name ?? id}
                  </ComboboxItem>
                )}
              </ComboboxList>
            </ComboboxContent>
          </Combobox>
        </Field>
        <Field>
          <FieldLabel>排序失败时</FieldLabel>
          <Select
            value={failure}
            onValueChange={(value) => {
              if (value === "reject" || value === "builtin") setFailure(value);
            }}
            disabled={locked}
          >
            <SelectTrigger aria-label="策略失败处理方式" />
            <SelectContent>
              <SelectItem value="reject">拒绝发送本轮请求</SelectItem>
              <SelectItem value="builtin">首次发送前使用内置策略</SelectItem>
            </SelectContent>
          </Select>
        </Field>
      </div>
      {selected && (
        <div className="space-y-3">
          <p className="break-words text-label text-fg-muted">
            {registryUnknown
              ? "当前绑定状态未知"
              : binding
                ? (binding.authorized ? "绑定已授权" : "绑定未授权") +
                  " · " +
                  binding.package.pluginId +
                  " · " +
                  binding.package.digest.slice(0, 12) +
                  " · " +
                  (binding.serviceVersionId === selected.versionId
                    ? "当前服务版本"
                    : "历史服务版本") +
                  " · " +
                  (binding.failureMode === "builtin"
                    ? "失败时使用内置策略"
                    : "失败时拒绝发送") +
                  (binding.errorCode ? " · " + binding.errorCode : "")
                : "当前未绑定外部策略"}
          </p>
          {scope?.pendingOperationId && (
            <p role="status" className="break-all text-body">
              发布结果待核查：{scope.pendingOperationId}
              。此服务暂不开始新的策略请求。
            </p>
          )}
          {registryUnknown && (
            <p role="alert">
              此服务的策略范围记录不可读取，请保留原文件核查；当前绑定状态未知。
            </p>
          )}
          {scope?.errorCode && (
            <p role="alert">
              绑定记录不可核查：{scope.errorCode}。请保留原文件。
            </p>
          )}
          <p className="text-label text-fg-muted">
            先在服务中启用优先级与回退，再从上表选择已启用的策略版本。失败处理仅适用于首次发送之前；已发送或结果不明的请求不会重新排序。
          </p>
          <div className="flex flex-wrap gap-2">
            <Button
              disabled={
                locked ||
                !selected.routingPolicy?.enabled ||
                !currentPackage?.active ||
                !!scope?.pendingOperationId ||
                !!scope?.errorCode ||
                registryUnknown
              }
              onClick={() => void run(() => bind(false))}
            >
              绑定当前服务版本
            </Button>
            <Button
              variant="tertiary"
              disabled={
                locked ||
                !selected.routingPolicy?.enabled ||
                !currentPackage?.active ||
                !!scope?.pendingOperationId ||
                !!scope?.errorCode ||
                registryUnknown
              }
              onClick={() => void run(() => bind(true))}
            >
              发布新版本并绑定
            </Button>
            <Button
              variant="tertiary"
              disabled={
                locked ||
                !binding ||
                !scope ||
                !!scope.pendingOperationId ||
                !!scope.errorCode ||
                registryUnknown
              }
              onClick={() =>
                void run(() =>
                  mutate({
                    kind: "unbind",
                    storeId,
                    serviceId: selected.id,
                    expectedBindingRevision: scope!.revision,
                  }),
                )
              }
            >
              解除绑定，使用内置策略
            </Button>
          </div>
        </div>
      )}
      {publications.length > 0 && (
        <DataTable
          table={publicationTable}
          aria-label="策略发布结果"
          paginationProps={{ labels: dataTablePagination }}
        />
      )}
      <RoutingPolicyRecovery storeId={storeId} />
      <Dialog
        open={!!resumePublication}
        onOpenChange={(open) => {
          if (!open && !busy) setResumePublication(null);
        }}
      >
        <DialogContent className="flex max-h-[85dvh] flex-col">
          <DialogHeader>
            <DialogTitle>继续原发布</DialogTitle>
            <DialogDescription>
              先核查原结果；已经提交时直接返回保存的结果。继续提交沿用该编号保存的原服务配置。
            </DialogDescription>
          </DialogHeader>
          <div className="min-h-0 space-y-3 overflow-y-auto break-all text-body">
            <p>操作编号：{resumePublication?.operationId}</p>
            <p>
              服务：
              {resumePublication?.request.action.kind === "save"
                ? resumePublication.request.action.draft.name
                : resumePublication?.serviceId}
            </p>
            {error && (
              <p role="alert" className="text-fg-danger">
                {error}
              </p>
            )}
            {!resumeRecord && (
              <p role="alert">原发布记录暂不可核查，请连接原后台后重试。</p>
            )}
            {resumeRecord &&
              ["committed", "cancelled"].includes(resumeRecord.state) && (
                <p role="status">原操作已结束，请关闭后核查原结果。</p>
              )}
          </div>
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setResumePublication(null)}
            >
              取消
            </Button>
            <Button
              loading={busy}
              disabled={
                locked ||
                !resumeRecord ||
                !["prepared", "committing", "unknown"].includes(
                  resumeRecord.state,
                )
              }
              onClick={() =>
                void run(async (isCurrent) => {
                  if (
                    !resumePublication ||
                    !resumeRecord ||
                    resumePublication.storeId !== storeId
                  )
                    throw Error("原发布范围已变化，请连接原后台核查。");
                  const reply = await routingPolicyApi.perform({
                    kind: "resume-publish",
                    storeId: resumePublication.storeId,
                    operationId: resumePublication.operationId,
                  });
                  if (reply?.kind !== "publish")
                    throw Error("原发布结果暂不可核查，请保留原编号。");
                  if (!["committed", "cancelled"].includes(reply.status.state))
                    throw Error("原发布结果仍未确认，请保留原编号继续核查。");
                  if (isCurrent()) setResumePublication(null);
                })
              }
            >
              按原编号继续提交
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!review}
        onOpenChange={(open) => {
          if (!open && !busy) setReview(null);
        }}
      >
        <DialogContent className="flex max-h-[85dvh] flex-col">
          <DialogHeader>
            <DialogTitle>核对策略插件包</DialogTitle>
            <DialogDescription>
              核对发布者及公钥后安装。纯策略插件不获得文件、网络或凭据访问权。
            </DialogDescription>
          </DialogHeader>
          {review && (
            <div className="min-h-0 space-y-3 overflow-y-auto break-all text-body">
              <p>
                {review.manifest.displayName} · {review.manifest.version}
              </p>
              <p>发布者：{review.manifest.publisher}</p>
              <p>公钥：{review.signingKey}</p>
              <p>摘要：{review.digest}</p>
            </div>
          )}
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setReview(null)}
            >
              取消
            </Button>
            <Button
              loading={busy}
              disabled={!review}
              onClick={() =>
                void run(async (isCurrent) => {
                  if (!review) return;
                  await mutate({
                    kind: "install",
                    storeId,
                    packageRef: review.packageRef,
                    digest: review.digest,
                    expectedSigningKey: review.signingKey,
                    approvePublisher: true,
                  });
                  if (isCurrent()) setReview(null);
                })
              }
            >
              信任此发布者并安装
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!confirmation}
        onOpenChange={(open) => {
          if (!open && !busy) setConfirmation(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {confirmation ? labels[confirmation.action] : "策略操作"}
            </DialogTitle>
            <DialogDescription>
              {confirmation?.item.manifest.displayName}
              。此操作会使原有授权或版本绑定需要重新核查，在途请求保持原发送归属。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setConfirmation(null)}
            >
              取消
            </Button>
            <Button
              loading={busy}
              onClick={() =>
                void run(async (isCurrent) => {
                  if (!confirmation) return;
                  await mutate({
                    kind: "state",
                    storeId,
                    package: {
                      pluginId: confirmation.item.manifest.id,
                      digest: confirmation.item.digest,
                    },
                    action: confirmation.action,
                  });
                  if (isCurrent()) setConfirmation(null);
                })
              }
            >
              确认
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
