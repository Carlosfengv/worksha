import { useId } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import type { useEditorCatalogSource } from "../hooks/use-editor-catalog-source";

export function ProviderCatalogSourceFields({
  source,
  disabled,
}: {
  source: ReturnType<typeof useEditorCatalogSource>;
  disabled: boolean;
}) {
  const consentId = useId(),
    privateId = useId();
  if (!source.expanded && !source.loading && !source.error)
    return (
      <Button type="button" variant="link" size="sm" onClick={source.open}>
        选择模型目录来源
      </Button>
    );
  const selected = source.choice
    ? `${source.choice.pluginId}@${source.choice.digest}`
    : "builtin";
  const options = [
    { value: "builtin", label: "内置模型目录" },
    ...source.plugins
      .filter((p) => !!p.digest)
      .map((p) => ({
        value: `${p.manifest.id}@${p.digest}`,
        label: `${p.manifest.displayName} · ${p.enabled ? "已启用" : "已停用"}`,
      })),
  ];
  if (!options.some((o) => o.value === selected))
    options.push({
      value: selected,
      label: `${source.choice?.pluginId} · 原版本暂不可用`,
    });
  const label = (value: string) =>
    options.find((o) => o.value === value)?.label ?? value;
  return (
    <div className="space-y-3">
      <Field>
        <FieldLabel>模型目录来源</FieldLabel>
        <Combobox
          items={options.map((o) => o.value)}
          value={selected}
          itemToStringLabel={label}
          disabled={disabled || source.loading}
          onValueChange={(value) => {
            if (!value) return;
            const plugin = source.plugins.find(
              (p) => `${p.manifest.id}@${p.digest}` === value,
            );
            if (value === "builtin") void source.choose(null);
            else if (plugin?.digest)
              void source.choose({
                pluginId: plugin.manifest.id,
                digest: plugin.digest,
              });
          }}
        >
          <ComboboxInput aria-label="模型目录来源" placeholder="搜索目录来源" />
          <ComboboxContent>
            <ComboboxEmpty>没有匹配的目录来源</ComboboxEmpty>
            <ComboboxList>
              {(value: string) => (
                <ComboboxItem key={value} value={value}>
                  {label(value)}
                </ComboboxItem>
              )}
            </ComboboxList>
          </ComboboxContent>
        </Combobox>
        <FieldDescription>
          来源与连接资料一起保存。外部目录失败时保留原请求，需核查后再操作。
        </FieldDescription>
      </Field>
      {source.loading && (
        <p role="status" className="text-body text-fg-muted">
          正在核对目录来源…
        </p>
      )}
      {source.error && (
        <Alert status="warning">
          <AlertTitle>目录来源尚不可用</AlertTitle>
          <AlertDescription>
            <p>{source.error}</p>
            <Button
              type="button"
              variant="tertiary"
              disabled={disabled || source.loading}
              onClick={source.retry}
            >
              重新核对目录来源
            </Button>
          </AlertDescription>
        </Alert>
      )}
      {source.state?.kind === "pending" && (
        <p role="status" className="text-body text-fg-muted">
          原目录操作仍待核查，请先处理服务商恢复列表。
        </p>
      )}
      {source.choice && (
        <>
          <p className="break-all text-body text-fg-muted">
            插件 {source.choice.pluginId} · 版本摘要{" "}
            {source.choice.digest.slice(0, 12)}。
            {source.policy
              ? `目录路径 ${source.policy.path}，使用 ${source.policy.baseProtocol} 协议；请在高级设置中核对协议和地址。`
              : "正在等待可用的目录策略。"}
          </p>
          {!source.policy && (
            <Button
              type="button"
              variant="tertiary"
              disabled={disabled || source.loading}
              onClick={() => void source.choose(source.choice)}
            >
              核对所选目录策略
            </Button>
          )}
          {source.state?.kind === "changed" && (
            <p className="text-body text-fg-muted">
              原目录范围已变化。请核对当前协议、地址与授权，再保存明确的来源选择。
            </p>
          )}
          <Field className="flex-row items-start gap-2">
            <Checkbox
              id={privateId}
              checked={source.privateNetwork}
              disabled={disabled || source.loading}
              onCheckedChange={(value) =>
                source.setPrivateNetwork(value === true)
              }
            />
            <FieldLabel htmlFor={privateId}>
              允许所选目录访问本机或私有网络地址
            </FieldLabel>
          </Field>
          <Field className="flex-row items-start gap-2">
            <Checkbox
              id={consentId}
              checked={source.consent}
              disabled={disabled || source.loading || !source.policy}
              onCheckedChange={(value) => source.setConsent(value === true)}
            />
            <FieldLabel htmlFor={consentId}>
              同意为本次连接授权所选目录读取模型；自动连接还会发送短验证请求
            </FieldLabel>
          </Field>
          <p className="text-body text-fg-muted">
            草稿授权只用于当前连接范围。保存后，需为已保存连接单独授权目录读取。
          </p>
          {source.state && (
            <Button
              type="button"
              variant="tertiary"
              disabled={
                disabled ||
                source.loading ||
                !source.policy ||
                !source.consent ||
                source.state.kind === "pending"
              }
              onClick={() => void source.authorizeSaved()}
            >
              授权当前已保存连接读取目录
            </Button>
          )}
        </>
      )}
    </div>
  );
}
