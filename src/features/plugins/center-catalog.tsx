import { useMemo, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  pluginScenarios,
  isPluginScenario,
  uploadedPluginPresentation,
  type PluginScenario,
} from "./scenarios";
import type { SystemFeature } from "./system-features";
import type { BuiltinPluginRow } from "./builtin-manager";
import { IAM_PLUGIN_ID } from "@/features/iam/types";
import { Badge } from "@/components/ui/badge";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import { useTablePagination } from "@/shared/hooks/use-table-pagination";
import { dataTablePagination } from "@/shared/ui/data-table-labels";
import type { PluginStatus } from "./types";
import type { Status as SourceStatus } from "../../../contracts/plugins/sources/types";
import type { PackageStatus as PolicyStatus } from "../../../contracts/plugins/routing/types";

export type CenterSelection = {
  kind: "system" | "plugin" | "source" | "policy";
  id: string;
};
export type CenterUserState = {
  kind: "plugin" | "source" | "policy";
  pluginId: string;
  digest: string;
  enabled: boolean;
};
type Row = {
  key: string;
  identity: CenterSelection;
  name: string;
  version: string;
  origin: "system" | "upload";
  capability: string;
  state: string;
  setup?: string | null;
  active: boolean;
  disabled: boolean;
  toggle?: (enabled: boolean) => void;
  href?: string;
  scenes: PluginScenario[];
  install?: () => void;
};
const columns: ColumnDef<Row>[] = [
  {
    id: "name",
    header: "插件",
    cell: ({ row }) => (
      <div className="max-w-64 whitespace-normal break-words">
        <p className="font-medium">{row.original.name}</p>
        <p className="text-label text-fg-muted">
          {row.original.scenes
            .map((scene) => pluginScenarios[scene].title)
            .join(" · ")}{" "}
          · {row.original.version}
        </p>
      </div>
    ),
  },
  {
    id: "origin",
    header: "来源",
    cell: ({ row }) => (
      <Badge>
        {row.original.origin === "system" ? "系统自带" : "用户上传"}
      </Badge>
    ),
  },
  {
    id: "capability",
    header: "功能",
    cell: ({ row }) => (
      <p className="max-w-64 whitespace-normal break-words text-label text-fg-muted">
        {row.original.capability}
      </p>
    ),
  },
  {
    id: "state",
    header: "当前状态",
    cell: ({ row }) => (
      <div className="space-y-1">
        <Badge
          status={
            row.original.active
              ? "success"
              : ["已停用", "待启用", "功能已提供"].includes(row.original.state)
                ? "neutral"
                : "warning"
          }
        >
          {row.original.state}
        </Badge>
        {row.original.setup && (
          <p className="max-w-48 whitespace-normal text-label text-fg-muted">
            {row.original.setup}
          </p>
        )}
      </div>
    ),
  },
  {
    id: "enabled",
    header: () => <span className="block px-3 text-right">操作</span>,
    cell: ({ row }) => (
      <div
        className="flex items-center justify-end gap-3"
        onClick={(event) => event.stopPropagation()}
      >
        {row.original.install ? (
          <Button
            size="sm"
            disabled={row.original.disabled}
            onClick={row.original.install}
          >
            安装并启用
          </Button>
        ) : (
          <Switch
            label={`启用 ${row.original.name}`}
            labelVisibility="sr-only"
            checked={row.original.active}
            disabled={row.original.disabled}
            onCheckedChange={row.original.toggle}
          />
        )}
        {row.original.href && (
          <Button size="sm" variant="tertiary" asChild>
            <a href={row.original.href}>打开功能</a>
          </Button>
        )}
      </div>
    ),
  },
];
const states: Record<string, string> = {
  installed: "待启用",
  awaiting_permission: "待授权",
  active: "运行中",
  disabled: "已停用",
  degraded: "部分能力失败",
  failed: "运行异常",
  removed: "已卸载",
};
export function PluginCenterCatalog({
  features,
  systemRows,
  initialScene,
  plugins,
  sources,
  policies,
  loading,
  uploadsUnavailable,
  sourceUnavailable,
  policyUnavailable,
  busy,
  onSelect,
  onUserToggle,
  selected,
}: {
  features: SystemFeature[];
  systemRows: BuiltinPluginRow[];
  initialScene?: string;
  plugins: PluginStatus[];
  sources: SourceStatus[];
  policies: PolicyStatus[];
  loading: boolean;
  uploadsUnavailable: boolean;
  sourceUnavailable: boolean;
  policyUnavailable: boolean;
  busy: boolean;
  onSelect: (value: CenterSelection) => void;
  onUserToggle: (value: CenterUserState) => void;
  selected: CenterSelection | null;
}) {
  const [scene, setScene] = useState<PluginScenario | "all">(
    initialScene && isPluginScenario(initialScene) ? initialScene : "all",
  );
  const [search, setSearch] = useState("");
  const [origin, setOrigin] = useState("all");
  const rows = useMemo<Row[]>(
    () =>
      [
        ...features.map((feature) => {
          const runtime = systemRows.find(
            (row) => row.plugin.id === feature.id,
          );
          return {
            key: `system:${feature.id}`,
            identity: { kind: "system" as const, id: feature.id },
            name: feature.name,
            version: feature.version ?? "随应用提供",
            origin: "system" as const,
            capability: feature.purpose,
            state: !runtime
              ? "状态待核查"
              : !runtime.installed
                ? "已卸载"
                : runtime.plugin.runtimeActive
                  ? "运行中"
                  : runtime.plugin.desiredEnabled
                    ? "运行异常"
                    : "已停用",
            setup: !runtime
              ? "尚未读取到运行状态"
              : runtime.locked
                ? "登录保护已开启，无法关闭或卸载"
                : feature.id === IAM_PLUGIN_ID && runtime.installed
                  ? "默认关闭，按需开启"
                  : null,
            active: !!runtime?.plugin.runtimeActive,
            disabled: busy || !runtime || runtime.disabled,
            ...(runtime && !runtime.installed
              ? { install: runtime.install }
              : { toggle: runtime?.toggle }),
            href: runtime?.plugin.runtimeActive ? feature.href : undefined,
            scenes: feature.scenes,
          };
        }),
        ...plugins.map((p) => ({
          key: `plugin:${p.digest}`,
          identity: { kind: "plugin" as const, id: p.digest },
          name: p.manifest.displayName,
          version: p.manifest.version,
          origin: "upload" as const,
          capability: uploadedPluginPresentation(p.manifest).purpose,
          scenes: uploadedPluginPresentation(p.manifest).scenes,
          state: states[p.runtimeStatus] ?? "状态未知",
          setup:
            p.manifest.permissions?.some(
              (permission) => permission.kind === "source.content.read",
            ) && !p.sourceAuthorized
              ? "数据文件待配置"
              : p.manifest.permissions?.some(
                    (permission) => permission.kind === "connection.request",
                  ) && !p.connectionAuthorized
                ? "连接待配置"
                : null,
          active: p.runtimeStatus === "active",
          disabled:
            busy ||
            uploadsUnavailable ||
            !Object.hasOwn(states, p.runtimeStatus) ||
            ["failed", "degraded", "removed"].includes(p.runtimeStatus),
          toggle: (enabled: boolean) =>
            onUserToggle({
              kind: "plugin",
              pluginId: p.manifest.id,
              digest: p.digest,
              enabled,
            }),
        })),
        ...sources.map((p) => ({
          key: `source:${p.digest}`,
          identity: { kind: "source" as const, id: p.digest },
          name: p.manifest.displayName,
          version: p.manifest.version,
          origin: "upload" as const,
          capability: "读取插件提供的项目、会话与活动记录",
          scenes: uploadedPluginPresentation(p.manifest).scenes,
          setup: p.sourceAuthorized
            ? p.freshness === "current"
              ? "数据已读取"
              : "数据待更新"
            : "数据文件待配置",
          state: p.runtimeActive
            ? "运行中"
            : p.enabled
              ? "待重新启用"
              : "已停用",
          active: p.runtimeActive,
          disabled: busy || sourceUnavailable,
          toggle: (enabled: boolean) =>
            onUserToggle({
              kind: "source",
              pluginId: p.manifest.id,
              digest: p.digest,
              enabled,
            }),
        })),
        ...policies.map((p) => ({
          key: `policy:${p.digest}`,
          identity: { kind: "policy" as const, id: p.digest },
          name: p.manifest.displayName,
          version: p.manifest.version,
          origin: "upload" as const,
          capability: "按模型服务给定的候选进行排序",
          scenes: uploadedPluginPresentation(p.manifest).scenes,
          setup: "具体模型服务绑定见详情",
          state: p.active ? "运行中" : p.enabled ? "待重新启用" : "已停用",
          active: p.active,
          disabled: busy || policyUnavailable,
          toggle: (enabled: boolean) =>
            onUserToggle({
              kind: "policy",
              pluginId: p.manifest.id,
              digest: p.digest,
              enabled,
            }),
        })),
      ].sort(
        (a, b) =>
          a.name.localeCompare(b.name, "zh-CN") || a.key.localeCompare(b.key),
      ),
    [
      features,
      systemRows,
      plugins,
      sources,
      policies,
      busy,
      uploadsUnavailable,
      sourceUnavailable,
      policyUnavailable,
      onUserToggle,
    ],
  );
  const filtered = rows.filter(
    (row) =>
      (scene === "all" || row.scenes.includes(scene)) &&
      (origin === "all" || row.origin === origin) &&
      `${row.name} ${row.capability}`
        .toLocaleLowerCase()
        .includes(search.trim().toLocaleLowerCase()),
  );
  const pagination = useTablePagination(filtered.length, 10);
  const { table } = useDataTable({
    data: filtered,
    columns,
    getRowId: (row) => row.key,
    enableSorting: false,
    enableRowSelection: false,
    enableHiding: false,
    autoResetPageIndex: false,
    state: { pagination: pagination.value },
    onPaginationChange: pagination.onChange,
  });
  return (
    <section className="space-y-3" aria-label="插件列表">
      <div className="flex flex-wrap gap-2" role="group" aria-label="功能场景">
        <Button
          size="sm"
          variant={scene === "all" ? "secondary" : "ghost"}
          aria-pressed={scene === "all"}
          onClick={() => {
            setScene("all");
            pagination.onChange({ ...pagination.value, pageIndex: 0 });
          }}
        >
          全部功能
        </Button>
        {(Object.keys(pluginScenarios) as PluginScenario[])
          .filter(
            (key) =>
              key !== "other" ||
              rows.some((row) => row.scenes.includes("other")),
          )
          .map((key) => (
            <Button
              key={key}
              size="sm"
              variant={scene === key ? "secondary" : "ghost"}
              aria-pressed={scene === key}
              onClick={() => {
                setScene(key);
                pagination.onChange({ ...pagination.value, pageIndex: 0 });
              }}
            >
              {pluginScenarios[key].title}
            </Button>
          ))}
      </div>
      {(scene === "all" || scene === "tasks") && (
        <Alert>
          <AlertTitle>任务普通插件待接入</AlertTitle>
          <AlertDescription>现有任务记录和配置保留。</AlertDescription>
        </Alert>
      )}
      {scene !== "all" && (
        <div className="flex flex-wrap items-center gap-3">
          <p className="text-label text-fg-muted">
            {pluginScenarios[scene].description}
          </p>
          {pluginScenarios[scene].href &&
            rows.some(
              (row) =>
                row.origin === "system" &&
                row.active &&
                row.href === pluginScenarios[scene].href,
            ) && (
              <Button variant="link" asChild>
                <a href={pluginScenarios[scene].href}>
                  {pluginScenarios[scene].action}
                </a>
              </Button>
            )}
        </div>
      )}
      <div className="flex flex-wrap items-center gap-3">
        <Input
          aria-label="搜索插件"
          placeholder="搜索名称或功能"
          value={search}
          onChange={(event) => {
            setSearch(event.target.value);
            pagination.onChange({ ...pagination.value, pageIndex: 0 });
          }}
          className="w-64"
        />
        <Select
          value={origin}
          onValueChange={(value) => {
            if (value) {
              setOrigin(value);
              pagination.onChange({ ...pagination.value, pageIndex: 0 });
            }
          }}
        >
          <SelectTrigger aria-label="插件来源" />
          <SelectContent>
            <SelectItem value="all">全部来源</SelectItem>
            <SelectItem value="system">系统自带</SelectItem>
            <SelectItem value="upload">用户上传</SelectItem>
          </SelectContent>
        </Select>
        <p className="text-label text-fg-muted">
          已列出 {rows.length} 项 · 当前 {filtered.length} 项
        </p>
      </div>
      <DataTable
        table={table}
        aria-label="插件中心列表"
        isLoading={loading && rows.length === 0}
        emptyMessage={
          uploadsUnavailable || sourceUnavailable || policyUnavailable
            ? "插件目录尚未完整读取，请核查上方提示"
            : "没有匹配的插件"
        }
        onRowActivate={(row) => onSelect(row.original.identity)}
        activeRowId={selected ? `${selected.kind}:${selected.id}` : null}
        showPagination={filtered.length > 10}
        paginationProps={{
          labels: dataTablePagination,
          pageSizeOptions: [10, 20, 50],
        }}
      />
    </section>
  );
}
