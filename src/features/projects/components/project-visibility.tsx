import { useCallback, useMemo, useState } from "react";
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
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Input } from "@/components/ui/input";
import { projectsApi } from "../api";
import type { HiddenProject } from "../types";
import { date } from "@/features/sessions/display";
import { dataTablePagination } from "@/shared/ui/data-table-labels";
import { useTablePagination } from "@/shared/hooks/use-table-pagination";

export type VisibilityChanged = (projects: HiddenProject[]) => Promise<void>;

export function HideProject({
  project,
  onChanged,
}: {
  project: { id: string; name: string; path: string };
  onChanged: VisibilityChanged;
}) {
  const [open, setOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      const result = await projectsApi.setHidden(project.id, true);
      await onChanged(result.hiddenProjects);
      setOpen(false);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setSaving(false);
    }
  };
  return (
    <>
      <Button
        variant="tertiary"
        size="sm"
        onClick={() => {
          setError(null);
          setOpen(true);
        }}
        aria-label={`隐藏项目 ${project.name}`}
      >
        隐藏
      </Button>
      <Dialog
        open={open}
        onOpenChange={(next) => {
          if (!saving) setOpen(next);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>隐藏此项目？</DialogTitle>
            <DialogDescription>
              隐藏后，项目列表、切换器和项目筛选不再显示它。原始
              Session、调用、历史用量和磁盘文件都保留，可随时在“已隐藏项目”中恢复。
            </DialogDescription>
          </DialogHeader>
          <div className="min-w-0 space-y-2">
            <p className="break-words font-medium">{project.name}</p>
            <p className="break-all text-label text-fg-muted">{project.path}</p>
            {error && (
              <Alert status="danger">
                <AlertTitle>隐藏未完成</AlertTitle>
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
          </div>
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={saving}
              onClick={() => setOpen(false)}
            >
              取消
            </Button>
            <Button loading={saving} onClick={() => void save()}>
              隐藏项目
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

export function HiddenProjects({
  items,
  onChanged,
}: {
  items: HiddenProject[];
  onChanged: VisibilityChanged;
}) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [saving, setSaving] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const restore = useCallback(
    async (id: string) => {
      setSaving(id);
      setError(null);
      try {
        const result = await projectsApi.setHidden(id, false);
        await onChanged(result.hiddenProjects);
      } catch (reason) {
        setError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        setSaving(null);
      }
    },
    [onChanged],
  );
  const columns = useMemo<ColumnDef<HiddenProject>[]>(
    () => [
      {
        id: "project",
        header: "项目",
        cell: ({ row }) => (
          <div className="max-w-96 space-y-1 whitespace-normal">
            <p className="break-words font-medium">{row.original.name}</p>
            <p className="break-all text-label text-fg-muted">
              {row.original.path}
            </p>
            <p className="text-label text-fg-subtle">
              隐藏于 {date(row.original.hiddenAt)}
            </p>
          </div>
        ),
      },
      {
        id: "action",
        header: "显示设置",
        cell: ({ row }) => (
          <Button
            variant="tertiary"
            size="sm"
            disabled={saving !== null}
            loading={saving === row.original.id}
            onClick={() => void restore(row.original.id)}
            aria-label={`恢复项目 ${row.original.name}`}
          >
            恢复显示
          </Button>
        ),
      },
    ],
    [saving, restore],
  );
  const filtered = items.filter((p) =>
    `${p.name} ${p.path}`.toLowerCase().includes(search.trim().toLowerCase()),
  );
  const pagination = useTablePagination(filtered.length, 20);
  const { table } = useDataTable({
    data: filtered,
    columns,
    getRowId: (row) => row.id,
    enableSorting: false,
    enableHiding: false,
    enableRowSelection: false,
    autoResetPageIndex: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
  });
  return (
    <>
      <Button
        variant="tertiary"
        onClick={() => {
          setSearch("");
          setError(null);
          setOpen(true);
        }}
      >
        已隐藏项目 · {items.length}
      </Button>
      <Dialog
        open={open}
        onOpenChange={(next) => {
          if (!saving) setOpen(next);
        }}
      >
        <DialogContent size="lg">
          <DialogHeader>
            <DialogTitle>已隐藏项目</DialogTitle>
            <DialogDescription>
              这里只管理项目显示，不删除或清理数据。恢复后仍按收录范围、Agent
              和日期筛选；会话来源断开时，重新接入后才会回到列表。
            </DialogDescription>
          </DialogHeader>
          <div
            className="max-h-96 space-y-3 overflow-y-auto"
            aria-label="已隐藏项目管理"
          >
            <Input
              aria-label="搜索已隐藏项目"
              placeholder="搜索名称或路径"
              value={search}
              onChange={(event) => {
                setSearch(event.target.value);
                pagination.onChange({ pageIndex: 0, pageSize: 20 });
              }}
            />
            {error && (
              <Alert status="danger">
                <AlertTitle>恢复未完成</AlertTitle>
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
            <DataTable
              table={table}
              role="region"
              aria-label="已隐藏项目列表"
              paginationProps={{ labels: dataTablePagination }}
              emptyMessage={
                items.length ? "没有匹配的已隐藏项目" : "没有已隐藏项目"
              }
            />
          </div>
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={saving !== null}
              onClick={() => setOpen(false)}
            >
              关闭
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
