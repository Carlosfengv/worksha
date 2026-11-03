import { useEffect, useId, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { ColumnDef } from "@tanstack/react-table";
import type { useCredentialOperations } from "../hooks/use-credential-operations";
import {
  discoveredCredentialKey,
  type CredentialDiscoveryPage,
  type CredentialDiscoveryRequest,
  type DiscoveredCredential,
} from "../credential-discovery";
import { message } from "../display";
import { DesktopError } from "@/shared/lib/desktop-command";

const sourceNames = {
  legacy: "旧版记录（归属待核对）",
  scoped_v2: "原后台的旧恢复空间",
  scoped_v3: "当前恢复空间",
};
const phaseNames = {
  pending: "保存结果待核查",
  ready: "记录标记已保存，仍需核查凭据",
  cancelled_pending: "已停止后续保存，仍待核查",
  cancelled: "记录已取消",
};
const issueNames = {
  invalid: "材料格式或来源无法核查",
  unavailable: "记录暂时无法读取，可重新查找",
  source_unverified: "请先恢复并核查原恢复材料",
  conflict: "当前恢复空间已有同编号操作；旧记录已保留，需人工核查",
};
interface DiscoveryRow {
  item: DiscoveredCredential;
  disabled: boolean;
  select: () => void;
}
const columns: ColumnDef<DiscoveryRow>[] = [
  {
    id: "label",
    header: "凭据记录",
    cell: ({ row }) => (
      <div className="max-w-64 space-y-1 break-words whitespace-normal">
        <p>
          {row.original.item.label ??
            (row.original.item.issue ? "无法核查的记录" : "已取消的凭据记录")}
        </p>
        <p className="break-all text-label text-fg-muted">
          {row.original.item.operationId}
        </p>
      </div>
    ),
  },
  {
    id: "source",
    header: "来源与阶段",
    cell: ({ row }) => {
      const item = row.original.item;
      return (
        <div className="max-w-64 space-y-1 whitespace-normal">
          <p>{sourceNames[item.kind]}</p>
          <p>{item.issue ? issueNames[item.issue] : phaseNames[item.phase!]}</p>
          {item.legacyStage && (
            <p>
              {item.legacyStage === "unclaimed"
                ? "历史归属需明确核对"
                : item.legacyStage === "committed"
                  ? "旧记录已迁移"
                  : "旧记录迁移尚未完成"}
            </p>
          )}
        </div>
      );
    },
  },
  {
    id: "action",
    header: "操作",
    cell: ({ row }) => (
      <Button
        type="button"
        size="sm"
        variant="secondary"
        disabled={row.original.disabled || !!row.original.item.issue}
        onClick={row.original.select}
      >
        添加恢复提示
      </Button>
    ),
  },
];
interface Listing {
  page: CredentialDiscoveryPage;
  request: CredentialDiscoveryRequest;
  context: number;
  previous: (string | null)[];
}
export function CredentialDiscovery({
  operations,
  disabled,
}: {
  operations: ReturnType<typeof useCredentialOperations>;
  disabled: boolean;
}) {
  const [open, setOpen] = useState(false),
    [listingValid, setListingValid] = useState(false),
    [listing, setListing] = useState<Listing | null>(null),
    [reading, setReading] = useState(false),
    [error, setError] = useState<string | null>(null),
    [notice, setNotice] = useState<string | null>(null),
    [selected, setSelected] = useState<DiscoveredCredential | null>(null),
    [confirmed, setConfirmed] = useState(false);
  const requestId = useRef(0),
    consentId = useId();
  useEffect(
    () => () => {
      requestId.current++;
    },
    [],
  );
  const current =
    !!listing &&
    listingValid &&
    operations.canDiscover &&
    listing.context === operations.contextToken &&
    listing.page.storeId === operations.storeId &&
    listing.page.namespaceId === operations.namespace?.namespaceId;
  const load = async (cursor: string | null, previous: (string | null)[]) => {
    if (disabled || !operations.canDiscover) return;
    const token = ++requestId.current,
      context = operations.contextToken;
    setReading(true);
    setListingValid(false);
    setError(null);
    setSelected(null);
    setConfirmed(false);
    try {
      const page = await operations.discover(cursor),
        offset = previous.length * 32;
      if (
        page.scanned !== Math.min(32, page.totalCandidates - offset) ||
        offset > page.totalCandidates ||
        !!page.nextCursor !== offset + page.scanned < page.totalCandidates
      )
        throw Error("后台分页信息无法核查，请重新查找。");
      if (requestId.current === token) {
        setListing({
          page,
          request: {
            storeId: page.storeId,
            namespaceId: page.namespaceId,
            cursor,
          },
          context,
          previous,
        });
        setListingValid(true);
      }
    } catch (reason) {
      if (requestId.current === token) setError(message(reason));
    } finally {
      if (requestId.current === token) setReading(false);
    }
  };
  const close = () => {
    requestId.current++;
    setOpen(false);
    setSelected(null);
    setConfirmed(false);
    setReading(false);
  };
  const rows: DiscoveryRow[] = (listing?.page.items ?? []).map((item) => ({
    item,
    disabled: disabled || operations.busy || reading || !current,
    select: () => {
      setSelected(item);
      setConfirmed(false);
      setError(null);
    },
  }));
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => discoveredCredentialKey(row.item),
    enableSorting: false,
    enableHiding: false,
    enableRowSelection: false,
    manualPagination: true,
    pageCount: -1,
    state: { pagination: { pageIndex: 0, pageSize: 32 } },
  });
  return (
    <>
      <Button
        type="button"
        variant="secondary"
        size="sm"
        disabled={disabled || operations.busy || !operations.canDiscover}
        onClick={() => {
          setNotice(null);
          setOpen(true);
          void load(null, []);
        }}
      >
        找回后台凭据记录
      </Button>
      {!operations.canDiscover && (
        <p className="mt-2 text-label text-fg-muted">
          核查当前后台和恢复空间后，可查找没有本机提示的记录。
        </p>
      )}
      {notice && (
        <p role="status" className="mt-2 text-body text-fg-muted">
          {notice}
        </p>
      )}
      <Dialog
        open={open}
        onOpenChange={(value) => {
          if (!value) close();
        }}
      >
        <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto">
          <DialogHeader>
            <DialogTitle>找回后台凭据记录</DialogTitle>
            <DialogDescription>
              只查看当前后台的保存记录，不读取 API
              Key，也不保存、取消或迁移凭据。添加本机提示后，仍需单独核查保存结果并明确采用。
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3 text-body text-fg-muted">
            {reading && <p role="status">正在查找后台记录…</p>}
            {error && !selected && (
              <p role="status">{error} 已有列表仅供参考，请重新查找后操作。</p>
            )}
            {listing && !current && (
              <p role="status">
                原后台或恢复空间已变化，已保留列表；重新查找后才能添加提示。
              </p>
            )}
            {listing && (
              <>
                <p>
                  共有 {listing.page.totalCandidates} 项可识别编号的保存材料，第{" "}
                  {listing.previous.length + 1} 页；本页检查{" "}
                  {listing.page.scanned} 项，显示 {listing.page.items.length}{" "}
                  条记录。
                </p>
                {listing.page.merged > 0 && (
                  <p>
                    本页 {listing.page.merged}{" "}
                    项旧材料已合并到当前空间的迁移记录，可继续翻页查看。
                  </p>
                )}
                {listing.page.unrecognized > 0 && (
                  <p role="status">
                    另有 {listing.page.unrecognized}{" "}
                    项材料名称无法识别，未修改；请保留原目录人工核查。
                  </p>
                )}
                {listing.page.items.length > 0 ? (
                  <DataTable table={table} showPagination={false} />
                ) : (
                  !reading &&
                  !error && (
                    <p>
                      {listing.page.totalCandidates === 0
                        ? listing.page.unrecognized
                          ? "没有可识别的操作记录；仍有材料待人工核查。"
                          : "当前后台未发现凭据保存记录。"
                        : "本页没有可恢复的独立记录，可继续查看其他页。"}
                    </p>
                  )
                )}
              </>
            )}
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="secondary"
              disabled={disabled || operations.busy || !operations.canDiscover}
              onClick={() => void load(null, [])}
            >
              重新查找
            </Button>
            <Button
              type="button"
              variant="secondary"
              disabled={
                disabled ||
                operations.busy ||
                !current ||
                !listing?.previous.length
              }
              onClick={() => {
                if (listing)
                  void load(
                    listing.previous.at(-1) ?? null,
                    listing.previous.slice(0, -1),
                  );
              }}
            >
              上一页
            </Button>
            <Button
              type="button"
              variant="secondary"
              disabled={
                disabled ||
                operations.busy ||
                !current ||
                !listing?.page.nextCursor
              }
              onClick={() => {
                if (listing?.page.nextCursor)
                  void load(listing.page.nextCursor, [
                    ...listing.previous,
                    listing.request.cursor,
                  ]);
              }}
            >
              下一页
            </Button>
            <Button type="button" variant="ghost" onClick={close}>
              关闭查找
            </Button>
          </DialogFooter>
          <Dialog
            open={!!selected}
            onOpenChange={(value) => {
              if (!value && !operations.busy) {
                setSelected(null);
                setConfirmed(false);
                setError(null);
              }
            }}
          >
            <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto">
              <DialogHeader>
                <DialogTitle>添加这条本机恢复提示</DialogTitle>
                <DialogDescription>
                  只添加提示，不读取或写入密钥，不重发连接或模型请求。添加前会重新核查原后台及记录内容。
                </DialogDescription>
              </DialogHeader>
              <div className="space-y-3 text-body text-fg-muted">
                <p className="break-words">
                  {selected?.label ?? "已取消的凭据记录"}
                </p>
                <p className="break-all">{selected?.operationId}</p>
                {selected?.kind === "legacy" && (
                  <p>
                    旧版记录没有历史后台身份。名称或编号不能证明归属，请确认它确实属于当前后台；添加提示不代表已确认迁移。
                  </p>
                )}
                {selected?.legacyStage &&
                  selected.legacyStage !== "committed" && (
                    <p>
                      添加后仍须在提示列表明确核对旧记录来源，再继续原迁移；不会自动创建新的凭据。
                    </p>
                  )}
                <label
                  htmlFor={consentId}
                  className="flex items-start gap-3 text-fg-default"
                >
                  <Checkbox
                    id={consentId}
                    checked={confirmed}
                    onCheckedChange={setConfirmed}
                    disabled={disabled || operations.busy || !current}
                  />
                  <span>
                    我已核对这条记录及当前后台，同意仅添加本机恢复提示。
                  </span>
                </label>
                {!current && (
                  <p role="status">
                    原后台或恢复空间已变化，请重新查找；未添加提示。
                  </p>
                )}
                {error && <p role="status">{error}</p>}
              </div>
              <DialogFooter>
                <Button
                  type="button"
                  variant="secondary"
                  disabled={operations.busy}
                  onClick={() => {
                    setSelected(null);
                    setConfirmed(false);
                    setError(null);
                  }}
                >
                  暂不添加
                </Button>
                <Button
                  type="button"
                  disabled={
                    disabled || operations.busy || !current || !confirmed
                  }
                  onClick={() => {
                    if (!confirmed || !selected || !listing || !current) return;
                    const token = requestId.current;
                    setError(null);
                    void operations
                      .restoreDiscovered(
                        listing.request,
                        listing.page,
                        selected,
                      )
                      .then(() => {
                        if (requestId.current !== token) return;
                        setNotice(
                          "已添加本机恢复提示。请核查保存结果或旧记录来源，再明确采用到连接。",
                        );
                        close();
                      })
                      .catch((reason) => {
                        if (requestId.current !== token) return;
                        if (reason instanceof DesktopError)
                          setListingValid(false);
                        setError(message(reason));
                      });
                  }}
                >
                  确认添加提示
                </Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>
        </DialogContent>
      </Dialog>
    </>
  );
}
