import { useCallback, useState } from "react";
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
import { projectsApi } from "./api";

export function ProjectPicker({
  value,
  onChange,
  disabled,
}: {
  value: string | null;
  onChange: (id: string | null) => void;
  disabled?: boolean;
}) {
  const [limit, setLimit] = useState(40);
  const [search, setSearch] = useState("");
  const loader = useCallback(async () => {
    const query = { scope: "all", query: search || undefined };
    let result = await projectsApi.projects(query);
    const seen = new Set<string>();
    while (result.nextCursor && result.items.length < limit) {
      if (seen.has(result.nextCursor))
        throw new Error("项目分页已失效，请重新读取");
      seen.add(result.nextCursor);
      const next = await projectsApi.projects({
        ...query,
        cursor: result.nextCursor,
      });
      result = { ...next, items: [...result.items, ...next.items] };
    }
    const selected =
      value && !result.items.some((item) => item.id === value)
        ? (await projectsApi.project(value, { scope: "all" })).project
        : null;
    return { ...result, selected };
  }, [search, limit, value]);
  const resource = usePollingResource(
    `task-projects:${search}:${limit}:${value}`,
    loader,
  );
  const options = new Map((resource.data?.items ?? []).map((p) => [p.id, p]));
  if (resource.data?.selected)
    options.set(resource.data.selected.id, resource.data.selected);
  const label = (id: string) => {
    const project = options.get(id);
    return project
      ? `${project.name} · ${project.path}${project.projectKind !== "workspace" ? " · 临时目录" : ""}`
      : resource.error
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
            {resource.loading
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
        {resource.data?.nextCursor && (
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
            改选其他文件夹
          </Button>
        )}
      </div>
      <p className="text-label text-fg-muted">
        项目来自已授权会话，包含临时目录。也可以直接选择本机已有文件夹，无需新建项目。发送前会通过系统选择器确认目录。
      </p>
    </div>
  );
}
