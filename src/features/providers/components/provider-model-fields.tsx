import type { Dispatch } from "react";
import { Button } from "@/components/ui/button";
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
export function ProviderModelFields({
  state,
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
                {fetching ? "正在获取模型…" : "获取模型"}
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
                    个已登记模型。选择停用模型为默认值时会同时启用。
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
          {advanced && (
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
