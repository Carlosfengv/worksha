import type { Dispatch, RefObject } from "react";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import type { ProviderDraftAction, ProviderEditorDraft } from "../editor-draft";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { presetDefaultEndpoint } from "../preset-advice";
import type { Provider } from "../types";
import { ProviderPresetField } from "./provider-preset-field";
import type { useProviderPresets } from "../hooks/use-provider-presets";
export function ProviderConnectionFields({
  state,
  dispatch,
  keyInput,
  busy,
  clearError,
  hasNewKey,
  onKeyChange,
  presets,
}: {
  state: ProviderEditorDraft;
  dispatch: Dispatch<ProviderDraftAction>;
  keyInput: RefObject<HTMLInputElement | null>;
  busy: boolean;
  clearError: () => void;
  hasNewKey: boolean;
  onKeyChange: (hasValue: boolean) => void;
  presets: ReturnType<typeof useProviderPresets>;
}) {
  const { provider: draft, endpointMode, advanced } = state;
  const automatic = state.mode === "automatic";
  const subscription = draft.vendor === "minimax-plan";
  return (
    <>
      {!automatic && (
        <ProviderPresetField
          state={state}
          presets={presets}
          disabled={!draft.id.startsWith("custom-")}
        />
      )}
      <Field>
        <FieldLabel>连接名称（可选）</FieldLabel>
        <Input
          maxLength={100}
          value={draft.name}
          onChange={(e) => dispatch({ type: "name", value: e.target.value })}
        />
      </Field>
      {!automatic && advanced && (
        <>
          <Field>
            <FieldLabel>接口协议</FieldLabel>
            <Select
              value={draft.protocol}
              onValueChange={(v) =>
                dispatch({ type: "protocol", value: v as Provider["protocol"] })
              }
            >
              <SelectTrigger aria-label="接口协议" />
              <SelectContent>
                <SelectItem value="responses">
                  OpenAI Responses · Codex
                </SelectItem>
                <SelectItem value="messages">
                  Anthropic Messages · Claude Code
                </SelectItem>
                <SelectItem value="chat-completions">
                  Chat Completions
                </SelectItem>
              </SelectContent>
            </Select>
            <FieldDescription>
              选择供应商实际提供的接口。Agent
              切换时自动匹配或转换协议，保存后可单独验证。
            </FieldDescription>
          </Field>
          {!subscription && (
            <Field>
              <FieldLabel>Endpoint 来源</FieldLabel>
              <Select
                value={endpointMode}
                onValueChange={(mode) =>
                  dispatch({
                    type: "endpoint-source",
                    value: mode as "default" | "custom",
                  })
                }
              >
                <SelectTrigger aria-label="Endpoint 来源" />
                <SelectContent>
                  <SelectItem
                    value="default"
                    disabled={!presetDefaultEndpoint(state)}
                  >
                    {state.preset?.origin.kind === "external"
                      ? "所选预设 Endpoint"
                      : "官方默认 Endpoint"}
                  </SelectItem>
                  <SelectItem value="custom">自定义 Endpoint</SelectItem>
                </SelectContent>
              </Select>
              <FieldDescription>
                {state.preset?.origin.kind === "external"
                  ? "预设地址只是初始值，可选择自定义并编辑；目录访问仍需单独授权。"
                  : draft.vendor === "ollama" || draft.vendor === "vllm"
                    ? "默认地址指本机服务地址；服务部署在其他位置时选择自定义。"
                    : "使用服务商官方地址，或填写代理、网关等自定义地址。"}
              </FieldDescription>
            </Field>
          )}
        </>
      )}
      <Field>
        <FieldLabel>
          {automatic ? "服务地址 URL" : advanced ? "Endpoint" : "服务地址"}
        </FieldLabel>
        <Input
          aria-label={
            automatic ? "服务地址 URL" : advanced ? "Endpoint" : "服务地址"
          }
          type="url"
          required
          readOnly={subscription}
          maxLength={2048}
          value={draft.endpoint}
          onChange={(e) =>
            dispatch({ type: "endpoint", value: e.target.value })
          }
        />
        <FieldDescription>
          {subscription
            ? "MiniMax M Plan 国际站官方地址。"
            : automatic
              ? "可填写服务地址或完整调用地址。"
              : "远程使用 HTTPS，本机回环可用 HTTP。"}
          地址中请勿填写密钥或认证参数。
        </FieldDescription>
      </Field>
      <Field>
        <FieldLabel>API Key</FieldLabel>
        <Input
          ref={keyInput}
          type="password"
          autoComplete="off"
          maxLength={8192}
          aria-label="API Key"
          onChange={(event) => onKeyChange(!!event.currentTarget.value)}
          placeholder={
            draft.credentialRef
              ? "已存入钥匙串；留空沿用"
              : "输入供应商提供的 API Key"
          }
        />
        <FieldDescription>
          {subscription
            ? "使用 MiniMax M Plan 控制台签发的 sk-cp- 订阅 Key。已保存的 Key 可留空沿用。"
            : hasNewKey
              ? "新 API Key 尚未保存；获取模型或保存连接后生效。"
              : draft.credentialRef
                ? `当前凭据：${draft.credentialLabel ?? "已保存"}。留空沿用。`
                : "只保存到 macOS 钥匙串；本机无鉴权服务可留空。"}
        </FieldDescription>
      </Field>

      {!automatic && advanced && (
        <Field>
          <FieldLabel>凭据名称</FieldLabel>
          <Input
            maxLength={100}
            value={state.label}
            onChange={(e) => dispatch({ type: "label", value: e.target.value })}
          />
        </Field>
      )}
      <div className="flex flex-wrap gap-3">
        <Button
          type="button"
          variant="link"
          size="sm"
          disabled={busy}
          onClick={() => {
            dispatch({ type: "advanced" });
            clearError();
          }}
        >
          {automatic || !advanced ? "高级设置" : "收起高级设置"}
        </Button>
        {!automatic && (
          <Button
            type="button"
            variant="link"
            size="sm"
            disabled={busy}
            onClick={() => {
              dispatch({ type: "mode", value: "automatic" });
              clearError();
            }}
          >
            自动识别并验证
          </Button>
        )}
      </div>
    </>
  );
}
