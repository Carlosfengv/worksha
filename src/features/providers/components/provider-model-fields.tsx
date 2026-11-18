import { useMemo, type Dispatch } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import type { ProviderDraftAction, ProviderEditorDraft } from "../editor-draft";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import { date } from "@/features/sessions/display";
import type { ProviderModelCatalog } from "../types";
import type { ColumnDef } from "@tanstack/react-table";

type CatalogModelRow = ProviderModelCatalog["models"][number] & {
  isDefault: boolean;
  included: boolean;
  setIncluded: (included: boolean) => void;
};

const catalogModelColumns: ColumnDef<CatalogModelRow>[] = [
  {
    id: "model",
    header: "模型 ID",
    cell: ({ row }) => (
      <div className="flex flex-wrap items-center gap-2">
        <span className="break-all">{row.original.id}</span>
        {row.original.isDefault && <Badge>默认</Badge>}
      </div>
    ),
  },
  {
    id: "include",
    header: "加入连接",
    cell: ({ row }) => (
      <Checkbox
        aria-label={`加入 ${row.original.id}`}
        checked={row.original.included}
        disabled={row.original.isDefault}
        onCheckedChange={row.original.setIncluded}
      />
    ),
  },
];

function CatalogModelChoices({
  catalog,
  draft,
  dispatch,
}: {
  catalog: ProviderModelCatalog;
  draft: ProviderEditorDraft;
  dispatch: Dispatch<ProviderDraftAction>;
}) {
  const selectedModel = draft.provider.model;
  const excludedModels = draft.excludedCatalogModels;
  const rows = useMemo(
    () =>
      catalog.models.map((model) => ({
        ...model,
        isDefault: model.id === selectedModel,
        included:
          model.id === selectedModel || !excludedModels.includes(model.id),
        setIncluded: (included: boolean) =>
          dispatch({
            type: "include-catalog-model",
            model: model.id,
            included,
          }),
      })),
    [catalog.models, dispatch, excludedModels, selectedModel],
  );
  const { table } = useDataTable({
    data: rows,
    columns: catalogModelColumns,
    getRowId: (row) => row.id,
    enableHiding: false,
    autoResetPageIndex: false,
    initialState: { pagination: { pageIndex: 0, pageSize: 5 } },
  });
  return (
    <DataTable
      table={table}
      role="region"
      aria-label="上游模型目录"
      showPagination={rows.length > 5}
      paginationProps={{ pageSizeOptions: [5, 10, 20, 50] }}
    />
  );
}

export function ProviderModelFields({
  state,
  guided = false,
  dispatch,
  activeCatalog,
  modelOptions,
  modelError,
  displayModel,
  fetching,
  saving,
  canReadModels,
  fetchModels,
}: {
  state: ProviderEditorDraft;
  guided?: boolean;
  dispatch: Dispatch<ProviderDraftAction>;
  activeCatalog: ProviderModelCatalog | null;
  modelOptions: Map<string, string>;
  modelError: string | null;
  displayModel: boolean;
  fetching: boolean;
  saving: boolean;
  canReadModels: boolean;
  fetchModels: () => Promise<void>;
}) {
  const { provider: draft, manualModel, advanced, catalog } = state;
  const automatic = state.mode === "automatic";
  return (
    <>
      {automatic &&
        (displayModel ? (
          <Field>
            <FieldLabel>指定模型 ID（可选）</FieldLabel>
            <Input
              maxLength={200}
              value={draft.model}
              onChange={(e) =>
                dispatch({ type: "model", value: e.target.value })
              }
              placeholder="目录无法读取时，填写服务商给出的模型 ID"
            />
            <FieldDescription>
              留空时从目录选择可调用模型，连接方式仍会自动识别。
            </FieldDescription>
          </Field>
        ) : (
          <Button
            type="button"
            variant="link"
            size="sm"
            onClick={() => dispatch({ type: "show-model", value: true })}
          >
            指定默认模型（可选）
          </Button>
        ))}
      {!automatic && (
        <>
          <div className="space-y-3">
            <div className="flex flex-wrap items-center gap-2">
              <Button
                type="button"
                variant="neutral"
                disabled={!canReadModels}
                onClick={() => void fetchModels()}
              >
                {fetching
                  ? "正在获取模型…"
                  : guided && activeCatalog
                    ? "刷新目录"
                    : "获取模型"}
              </Button>
              <Button
                type="button"
                variant="tertiary"
                disabled={fetching || saving}
                onClick={() =>
                  dispatch({ type: "manual-model", value: !manualModel })
                }
              >
                {manualModel ? "从目录选择" : "手动输入模型 ID"}
              </Button>
            </div>
            {guided && activeCatalog && activeCatalog.models.length > 0 && (
              <CatalogModelChoices
                key={activeCatalog.fetchedAt}
                catalog={activeCatalog}
                draft={state}
                dispatch={dispatch}
              />
            )}

            <Field>
              <FieldLabel>默认模型</FieldLabel>
              {!manualModel && (
                <>
                  <Combobox<string>
                    items={[...modelOptions.keys()]}
                    value={draft.model || null}
                    required
                    limit={200}
                    itemToStringLabel={(id) =>
                      modelOptions.get(id) === id
                        ? id
                        : `${modelOptions.get(id)} · ${id}`
                    }
                    onValueChange={(model) => {
                      if (model && modelOptions.has(model))
                        dispatch({ type: "select-model", value: model });
                    }}
                  >
                    <ComboboxInput
                      aria-label="默认模型"
                      placeholder={
                        modelOptions.size
                          ? "搜索模型名称或 ID"
                          : "先获取模型，再搜索选择"
                      }
                      triggerAriaLabel="展开模型目录"
                    />
                    <ComboboxContent>
                      <ComboboxEmpty>
                        没有匹配的模型，可切换到手动输入。
                      </ComboboxEmpty>
                      <ComboboxList>
                        {(id: string) => (
                          <ComboboxItem key={id} value={id}>
                            {modelOptions.get(id) === id
                              ? id
                              : `${modelOptions.get(id)} · ${id}`}
                            {draft.disabledModels?.includes(id)
                              ? " · 已停用"
                              : ""}
                          </ComboboxItem>
                        )}
                      </ComboboxList>
                    </ComboboxContent>
                  </Combobox>
                  <FieldDescription>
                    {modelOptions.size}{" "}
                    {guided
                      ? "个可选模型。"
                      : "个已登记模型。选择停用模型为默认值时会同时启用。"}
                  </FieldDescription>
                </>
              )}
              {manualModel && (
                <Input
                  required
                  maxLength={200}
                  value={draft.model}
                  onChange={(e) =>
                    dispatch({ type: "model", value: e.target.value })
                  }
                  placeholder="服务商提供的模型 ID"
                />
              )}
              <FieldDescription>
                仅获取目录。调用权限需单独验证。
                {activeCatalog &&
                  ` 已获取 ${activeCatalog.models.length} 个模型 · ${date(activeCatalog.fetchedAt)}。`}
                {activeCatalog?.truncated &&
                  "目录较大，仅加载部分模型；仍可手动补充。"}
                {activeCatalog?.models.length === 0 &&
                  "服务商返回空目录；已有模型保留，可手动填写。"}
              </FieldDescription>
              {modelError && (
                <Alert status="warning">
                  <AlertTitle>未能获取模型</AlertTitle>
                  <AlertDescription>
                    {modelError} 已有模型保持不变，可重试或手动输入。
                  </AlertDescription>
                </Alert>
              )}
            </Field>
          </div>
          {(advanced || guided) && (
            <Field>
              <FieldLabel>补充模型目录</FieldLabel>
              <Input
                maxLength={400000}
                value={catalog}
                onChange={(e) =>
                  dispatch({ type: "catalog", value: e.target.value })
                }
                placeholder="多个模型 ID 用逗号分隔"
              />
              <FieldDescription>
                可手动补充模型
                ID。保存时合并已获取目录，更改默认模型会保留其他可选模型。
              </FieldDescription>
            </Field>
          )}
        </>
      )}
    </>
  );
}
