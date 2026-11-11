import { useCallback, useState, useSyncExternalStore } from "react";
import { Button } from "@/components/ui/button";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import { usePollingResource } from "@/shared/hooks/use-polling-resource";
import {
  capabilityAccessEpoch,
  subscribeCapabilityAccess,
} from "@/shared/lib/capability-access";
import { projectsApi } from "./api";
import type { RoutingProjectPage, RoutingProjectQuery } from "./types";

export function ProjectPicker({
  value,
  onChange,
  disabled,
  loadRoutingProjects,
  hint = "项目来自已授权会话，包含临时目录。也可以直接选择本机已有文件夹，无需新建项目。发送前会通过系统选择器确认目录。",
  clearLabel = "改选其他文件夹",
}: {
  value: string | null;
  onChange: (id: string | null) => void;
  disabled?: boolean;
  loadRoutingProjects?: (
    query: RoutingProjectQuery,
  ) => Promise<RoutingProjectPage>;
  hint?: string;
  clearLabel?: string;
}) {
  const [limit, setLimit] = useState(40);
  const [search, setSearch] = useState("");
  const access = useSyncExternalStore(
    subscribeCapabilityAccess,
    capabilityAccessEpoch,
  );
  const loader = useCallback(async () => {
    const query = { scope: "all", query: search || undefined };
    const load = async (cursor?: string): Promise<RoutingProjectPage> => {
      if (loadRoutingProjects) {
        return loadRoutingProjects({
          query: query.query,
          cursor,
          selectedId: value ?? undefined,
        });
      }
      const result = await projectsApi.projects({ ...query, cursor });
      return {
        items: result.items,
        nextCursor: result.nextCursor,
        selected: null,
      };
    };
    let result = await load();
    const seen = new Set<string>();
    while (result.nextCursor && result.items.length < limit) {
      if (seen.has(result.nextCursor))
        throw new Error("项目分页已失效，请重新读取");
      seen.add(result.nextCursor);
      const next = await load(result.nextCursor);
      result = { ...next, items: [...result.items, ...next.items] };
    }
    const selected =
      !loadRoutingProjects &&
      value &&
      !result.items.some((item) => item.id === value)
        ? (await projectsApi.project(value, { scope: "all" })).project
        : result.selected;
    return { ...result, selected, access };
  }, [access, loadRoutingProjects, search, limit, value]);
  const resource = usePollingResource(
    `${loadRoutingProjects ? "routing" : "task"}-projects:${search}:${limit}:${value}`,
    loader,
  );
  const data = resource.data?.access === access ? resource.data : null;
  const loading = resource.loading || (!data && !resource.error);
  const options = new Map((data?.items ?? []).map((p) => [p.id, p]));
  if (data?.selected) options.set(data.selected.id, data.selected);
  const label = (id: string) => {
    const project = options.get(id);
    return project
      ? `${project.name} · ${project.path}${project.projectKind !== "workspace" ? " · 临时目录" : ""}`
      : resource.error || data
        ? "所选项目暂不可用"
        : "正在读取所选项目";
  };
  return (
    <div className="space-y-2">
      <Combobox<string>
        items={[...options.keys()]}
        value={value}
        itemToStringLabel={label}
        filter={null}
        disabled={disabled}
        onInputValueChange={(text, details) => {
          if (
            details.reason === "input-change" ||
            details.reason === "input-clear"
          ) {
            setSearch(text);
            setLimit(40);
          }
        }}
        onValueChange={onChange}
      >
        <ComboboxInput
          aria-label="已有项目"
          placeholder="搜索已有项目名称或路径"
          triggerAriaLabel="展开已有项目"
        />
        <ComboboxContent>
          <ComboboxEmpty>
            {loading
              ? "正在读取项目…"
              : resource.error
                ? "项目读取失败，可在下方重试"
                : "没有匹配的已授权项目"}
          </ComboboxEmpty>
          <ComboboxList>
            {(id: string) => (
              <ComboboxItem key={id} value={id}>
                {label(id)}
              </ComboboxItem>
            )}
          </ComboboxList>
        </ComboboxContent>
      </Combobox>
      {resource.error && (
        <p role="alert" className="text-label text-fg-muted">
          {resource.error}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        {data?.nextCursor && (
          <Button
            variant="tertiary"
            disabled={disabled}
            loading={resource.loading}
            onClick={() => setLimit((n) => n + 40)}
          >
            加载更多项目
          </Button>
        )}
        <Button
          variant="tertiary"
          disabled={disabled}
          loading={resource.loading}
          onClick={() => void resource.reload()}
        >
          重新读取项目
        </Button>
        {value && (
          <Button
            variant="tertiary"
            disabled={disabled}
            onClick={() => onChange(null)}
          >
            {clearLabel}
          </Button>
        )}
      </div>
      <p className="text-label text-fg-muted">{hint}</p>
    </div>
  );
}
