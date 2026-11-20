import { useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  DetailList,
  DetailListItem,
  DetailListLabel,
  DetailListValue,
} from "@/components/ui/detail-list";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { useIcon } from "@/lib/icon-context";
import type {
  CollectionDef,
  PlatformAction,
  PlatformRecord,
  PlatformViewDef,
} from "../../../../contracts/plugins/platform/types";
import { RecordForm, type RecordValues } from "./record-form";

type Act = (
  action: PlatformAction,
  values: RecordValues,
  recordId?: string,
  expectedValues?: RecordValues,
) => Promise<boolean>;

function text(value: unknown) {
  if (typeof value === "boolean") return value ? "是" : "否";
  return typeof value === "string" || typeof value === "number"
    ? String(value)
    : "—";
}

function FieldValue({
  record,
  field,
}: {
  record: PlatformRecord;
  field: string;
}) {
  const entity = record._hydrated[field];
  const value = record.values[field];
  if (entity?.authorization === "unauthorized")
    return <Badge variant="plain">摘要未授权</Badge>;
  if (entity?.freshness === "unknown")
    return <Badge variant="plain">关联摘要未知</Badge>;
  return entity ? (
    <span className="flex flex-wrap items-center gap-1.5">
      {entity.title || text(value)}
      {entity.activityState && (
        <Badge variant="plain">{entity.activityState}</Badge>
      )}
      {entity.model && <Badge variant="plain">{entity.model}</Badge>}
      {entity.totalTokens != null && (
        <Badge variant="plain">
          {entity.totalTokens.toLocaleString()} Token
        </Badge>
      )}
    </span>
  ) : (
    text(value)
  );
}

function RecordsTable({
  collection,
  records,
  onEdit,
}: {
  collection: CollectionDef;
  records: PlatformRecord[];
  onEdit?: (record: PlatformRecord) => void;
}) {
  const { table } = useDataTable<PlatformRecord>({
    data: records,
    columns: Object.entries(collection.fields).map(([id, field]) => ({
      id,
      header: field.label,
      cell: ({ row }) => <FieldValue record={row.original} field={id} />,
    })),
    getRowId: (record) => record.recordId,
    manualPagination: true,
    manualSorting: true,
  });
  return (
    <DataTable
      table={table}
      showPagination={false}
      emptyMessage="暂无记录"
      onRowActivate={onEdit ? (row) => onEdit(row.original) : undefined}
    />
  );
}

function sessionHref(record: PlatformRecord, field: string) {
  const path = `#/sessions/${encodeURIComponent(String(record.values[field]))}`;
  const source = record._hydrated[field]?.sourceId;
  return source
    ? `${path}?scope=unified&source=${encodeURIComponent(source)}`
    : path;
}

export function CollectionBlock({
  view,
  collection,
  records,
  action,
  compact,
  binding,
  next,
  loadMore,
  busy,
  error,
}: {
  view: PlatformViewDef;
  collection: CollectionDef;
  records: PlatformRecord[];
  action: Act;
  compact: boolean;
  binding?: { field: string; value: string };
  next?: string | null;
  loadMore: () => void;
  busy: boolean;
  error?: string;
}) {
  const [edit, setEdit] = useState<{
    record?: PlatformRecord;
    initial: RecordValues;
  }>();
  const [deleting, setDeleting] = useState<PlatformRecord>();
  const Add = useIcon("plus");
  const group = collection.groupBy;
  const columns = group ? (collection.fields[group]?.options ?? []) : [];
  const primary = view.primaryField ?? Object.keys(collection.fields)[0];
  const sessionField = Object.keys(collection.fields).find(
    (key) => collection.fields[key].kind === "ref:session",
  );
  const allows = (id: PlatformAction) => view.actions?.includes(id) ?? false;
  const add = (column = columns[0]) =>
    setEdit({
      initial: {
        ...(group && column ? { [group]: column } : {}),
        ...(binding ? { [binding.field]: binding.value } : {}),
      },
    });
  const editRecord = (record: PlatformRecord) =>
    setEdit({ record, initial: record.values });
  return (
    <section
      aria-label={view.title}
      className={compact ? "inline-flex" : "min-w-0 space-y-3"}
    >
      {!compact && (
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 className="text-title font-medium">{view.title}</h3>
          {view.kind === "stat-row" && (
            <Badge variant="plain">
              {records.length}
              {next ? "+" : ""} 条记录
            </Badge>
          )}
        </div>
      )}
      {allows("records.create") && (
        <Button
          size="sm"
          variant="secondary"
          leadingIcon={Add}
          disabled={busy}
          onClick={() => add()}
        >
          {compact ? view.title : "新建记录"}
        </Button>
      )}
      {view.kind === "kanban" && group && (
        <div className="grid auto-cols-[minmax(13rem,1fr)] grid-flow-col gap-3 overflow-x-auto pb-2">
          {columns.map((column) => {
            const cards = records.filter(
              (record) => record.values[group] === column,
            );
            return (
              <section
                key={column}
                aria-label={column}
                className="space-y-2 rounded-lg border border-border bg-surface p-3"
              >
                <div className="flex items-center justify-between gap-2">
                  <h4 className="text-label font-medium">{column}</h4>
                  <Badge variant="plain">
                    {cards.length}
                    {next ? "+" : ""}
                  </Badge>
                </div>
                {cards.map((record) => (
                  <Card key={record.recordId}>
                    <CardHeader>
                      <CardTitle className="break-words">
                        {text(record.values[primary])}
                      </CardTitle>
                    </CardHeader>
                    <CardContent className="space-y-2">
                      {sessionField && record.values[sessionField] != null && (
                        <div className="space-y-1 text-label">
                          <FieldValue record={record} field={sessionField} />
                          {record._hydrated[sessionField]?.authorization ===
                            "authorized" && (
                            <a
                              data-workspace-pin
                              data-workspace-title={
                                record._hydrated[sessionField]?.title
                              }
                              href={sessionHref(record, sessionField)}
                              className="text-fg-link underline"
                            >
                              打开关联会话
                            </a>
                          )}
                        </div>
                      )}
                      {allows("records.move") && (
                        <Select
                          size="sm"
                          value={column}
                          onValueChange={(value) => {
                            if (value && value !== column)
                              void action(
                                "records.move",
                                { [group]: value },
                                record.recordId,
                                { [group]: record.values[group] ?? null },
                              );
                          }}
                        >
                          <SelectTrigger
                            aria-label={`移动 ${text(record.values[primary])}`}
                            disabled={busy}
                          />
                          <SelectContent>
                            {columns.map((target) => (
                              <SelectItem key={target} value={target}>
                                {target}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      )}
                      <div className="flex flex-wrap gap-1">
                        {allows("records.update") && (
                          <Button
                            size="sm"
                            variant="tertiary"
                            onClick={() => editRecord(record)}
                          >
                            编辑
                          </Button>
                        )}
                        {allows("records.delete") && (
                          <Button
                            size="sm"
                            variant="tertiary"
                            onClick={() => setDeleting(record)}
                          >
                            删除
                          </Button>
                        )}
                      </div>
                    </CardContent>
                  </Card>
                ))}
                {allows("records.create") && (
                  <Button
                    size="sm"
                    variant="tertiary"
                    disabled={busy}
                    onClick={() => add(column)}
                  >
                    添加记录
                  </Button>
                )}
              </section>
            );
          })}
        </div>
      )}
      {view.kind === "data-table" && (
        <RecordsTable
          collection={collection}
          records={records}
          onEdit={allows("records.update") ? editRecord : undefined}
        />
      )}
      {view.kind === "detail-list" &&
        (records.length === 0 ? (
          <p className="text-body text-fg-muted">暂无关联记录</p>
        ) : (
          records.map((record) => (
            <Card key={record.recordId}>
              <CardHeader>
                <CardTitle>{text(record.values[primary])}</CardTitle>
              </CardHeader>
              <CardContent>
                <DetailList>
                  {Object.entries(collection.fields)
                    .filter(([key]) => key !== primary)
                    .map(([key, field]) => (
                      <DetailListItem key={key}>
                        <DetailListLabel>{field.label}</DetailListLabel>
                        <DetailListValue>
                          <FieldValue record={record} field={key} />
                        </DetailListValue>
                      </DetailListItem>
                    ))}
                </DetailList>
                <div className="flex flex-wrap items-center gap-2">
                  {allows("records.move") && group && (
                    <Select
                      size="sm"
                      value={text(record.values[group])}
                      onValueChange={(value) => {
                        if (value)
                          void action(
                            "records.move",
                            { [group]: value },
                            record.recordId,
                            { [group]: record.values[group] ?? null },
                          );
                      }}
                    >
                      <SelectTrigger
                        aria-label={`移动 ${text(record.values[primary])}`}
                        disabled={busy}
                      />
                      <SelectContent>
                        {columns.map((column) => (
                          <SelectItem key={column} value={column}>
                            {column}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  )}
                  {allows("records.update") && (
                    <Button
                      size="sm"
                      variant="tertiary"
                      onClick={() => editRecord(record)}
                    >
                      编辑
                    </Button>
                  )}
                </div>
              </CardContent>
            </Card>
          ))
        ))}
      {!compact && next && (
        <Button size="sm" variant="secondary" onClick={loadMore}>
          加载更多
        </Button>
      )}
      {edit && (
        <RecordForm
          title={edit.record ? "编辑记录" : "新建记录"}
          collection={collection}
          initial={edit.initial}
          primaryField={primary}
          lockedField={binding?.field}
          error={error}
          onClose={() => setEdit(undefined)}
          onSave={(values) => {
            if (!edit.record) return action("records.create", values);
            const changes = Object.entries(values).filter(
              ([key, value]) => (value ?? null) !== (edit.initial[key] ?? null),
            );
            if (changes.length === 0) return Promise.resolve(true);
            return action(
              "records.update",
              Object.fromEntries(changes),
              edit.record.recordId,
              Object.fromEntries(
                changes.map(([key]) => [key, edit.initial[key] ?? null]),
              ),
            );
          }}
        />
      )}
      {deleting && (
        <Dialog
          open
          onOpenChange={(open) => {
            if (!open && !busy) setDeleting(undefined);
          }}
        >
          <DialogContent>
            <DialogTitle>删除记录</DialogTitle>
            <DialogDescription>
              删除「{text(deleting.values[primary])}」及其关联。
            </DialogDescription>
            <DialogFooter>
              <Button
                variant="secondary"
                disabled={busy}
                onClick={() => setDeleting(undefined)}
              >
                取消
              </Button>
              <Button
                variant="destructive"
                loading={busy}
                onClick={async () => {
                  if (await action("records.delete", {}, deleting.recordId))
                    setDeleting(undefined);
                }}
              >
                删除
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}
    </section>
  );
}
