import { Button } from "@/components/ui/button";
import {
  Combobox,
  ComboboxInput,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { presetKey, presetLabel } from "../preset-advice";
import type { ProviderEditorDraft } from "../editor-draft";
import type { useProviderPresets } from "../hooks/use-provider-presets";

export function ProviderPresetField({
  state,
  presets,
  disabled,
}: {
  state: ProviderEditorDraft;
  presets: ReturnType<typeof useProviderPresets>;
  disabled: boolean;
}) {
  const choices = new Map(presets.items.map((p) => [presetKey(p.origin), p]));
  const available = new Set(choices.keys());
  // A copied template can disappear from availability without erasing the
  // user's draft or silently changing its displayed source to a builtin.
  if (state.preset && !choices.has(presetKey(state.preset.origin)))
    choices.set(presetKey(state.preset.origin), state.preset);
  return (
    <Field>
      <FieldLabel>供应商</FieldLabel>
      <Combobox<string>
        items={[...choices.keys()]}
        value={
          state.vendorChosen && state.preset
            ? presetKey(state.preset.origin)
            : null
        }
        disabled={disabled || presets.resolving}
        limit={200}
        itemToStringLabel={(key) =>
          choices.has(key)
            ? `${presetLabel(choices.get(key)!)}${available.has(key) ? "" : "（已复制，来源待核查）"}`
            : ""
        }
        onOpenChange={(open) => {
          if (open) void presets.read();
        }}
        onValueChange={(key) => {
          if (key) void presets.choose(key);
        }}
      >
        <ComboboxInput
          aria-label="供应商"
          placeholder="选择供应商"
          triggerAriaLabel="展开供应商预设"
        />
        <ComboboxContent>
          <ComboboxEmpty>
            {presets.loading
              ? "正在读取预设…"
              : "没有匹配的预设，可选择其他供应商并手动填写。"}
          </ComboboxEmpty>
          <ComboboxList>
            {(key: string) => (
              <ComboboxItem
                key={key}
                value={key}
                disabled={!available.has(key)}
              >
                {presetLabel(choices.get(key)!)}
                {available.has(key) ? "" : "（已复制，来源待核查）"}
              </ComboboxItem>
            )}
          </ComboboxList>
        </ComboboxContent>
      </Combobox>
      {presets.loading && (
        <FieldDescription role="status">
          正在读取已启用插件的预设；草稿保持。
        </FieldDescription>
      )}
      {presets.resolving && (
        <FieldDescription role="status">
          正在核查所选预设，尚未替换草稿。
        </FieldDescription>
      )}
      {(presets.listError || presets.issues > 0 || presets.selectionError) && (
        <FieldDescription role="status">
          {presets.listError
            ? `外部预设暂不可读取：${presets.listError} 内置预设可继续选择。`
            : presets.issues > 0
              ? `${presets.issues} 个外部来源暂不可用，其余预设可继续选择。`
              : "可重新读取当前可用的供应商预设。"}
          <Button
            type="button"
            variant="link"
            size="sm"
            disabled={disabled || presets.loading || presets.resolving}
            onClick={() => void presets.read(true)}
          >
            重新读取供应商预设
          </Button>
        </FieldDescription>
      )}
      {presets.selectionError && (
        <FieldDescription role="alert">
          {presets.selectionError} 原草稿保持。
        </FieldDescription>
      )}
      {state.preset?.origin.kind === "external" && (
        <FieldDescription>
          已复制 {state.preset.sourceName}{" "}
          的预设初始值；可继续编辑，不代表连接已验证。目录来源与访问授权单独选择。
        </FieldDescription>
      )}
    </Field>
  );
}
