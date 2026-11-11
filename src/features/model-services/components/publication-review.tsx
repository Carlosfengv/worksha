import { useState } from "react";
import { routingQuotaLabels } from "@/features/accounts/display";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldLabel } from "@/components/ui/field";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import {
  cooldownLabels,
  recoveryLabels,
  verificationLabels,
} from "@/features/providers/upstream-status";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import { formatDateTime } from "@/shared/lib/date-time";
import { servicePreviewResource } from "../queries";
import type {
  ModelServiceDraft,
  PreviewModel,
  ServicePreviewEntry,
  ServicePreviewRequest,
} from "../types";

const modelStates: Record<string, string> = {
  providerMissing: "供应商不存在",
  providerUnconfigured: "供应商尚未配置",
  modelMissing: "尚未选择上游模型",
  modelUnavailable: "上游模型已停用或不在目录中",
  invalidSource: "默认连接配置无效",
  sourceChanged: "当前供应商与子服务发布快照不同",
  childLimit: "子服务超出深度、循环或候选数量限制",
  childUnavailable: "固定的子服务版本不可用",
  childCapabilitiesChanged: "子服务能力与草稿声明不同",
  candidateLimit: "超出账号池或展开后的候选数量限制",
};
const accountStates: Record<string, string> = {
  unknown: "账号目录无法核查",
  missing: "账号已找不到",
  revoked: "账号已撤销",
  changed: "账号版本已变化",
  paused: "账号已暂停",
  sourceUnavailable: "上游连接不可用",
  sourceMismatch: "账号与当前端点或协议不匹配",
};
const scopeLabels: Record<string, string> = {
  preferred: "入口或规则目标",
  fallback: "允许回退",
  disabled: "模型已停用",
  routingOff: "自动路由未开启",
  capabilityMismatch: "能力与此入口不匹配",
  outsideFallback: "跨模型回退未开启",
  entryUnavailable: "入口待核查",
};
const quotaLabels: Record<string, string> = {
  ...routingQuotaLabels,
  reported: "已有额度快照",
};

function CandidateModels({ models }: { models: PreviewModel[] }) {
  return (
    <ul className="min-w-0 space-y-3">
      {models.map((model) => (
        <li
          key={model.id}
          className="min-w-0 space-y-2 border-b border-border pb-3 last:border-0 last:pb-0"
        >
          <div className="flex min-w-0 flex-wrap items-start gap-x-2 gap-y-1">
            <p className="min-w-0 break-all text-body font-medium">
              {model.name || "未命名模型"}
            </p>
            <Badge
              status={
                model.scope === "preferred" || model.scope === "fallback"
                  ? "info"
                  : "neutral"
              }
            >
              {scopeLabels[model.scope] ?? "范围待核查"}
            </Badge>
          </div>
          {(model.source || model.model) && (
            <p className="break-all text-label text-fg-muted">
              {[model.source, model.model].filter(Boolean).join(" / ")}
            </p>
          )}
          {model.state !== "configured" && (
            <p className="break-words text-label text-fg-muted">
              {modelStates[model.state] ?? "来源待核查"}
            </p>
          )}
          {!!model.accounts.length && (
            <ol className="space-y-2 pl-3">
              {model.accounts.map((account) => (
                <li
                  key={account.position ?? "default"}
                  className="min-w-0 break-all text-label"
                >
                  <span className="text-fg-default">
                    {account.position
                      ? `${account.position}. ${account.label ?? "上游账号"}`
                      : "默认连接"}
                  </span>
                  {account.state === "configured" ? (
                    <p className="text-fg-muted">
                      {verificationLabels[account.verification] ??
                        "验证状态未知"}
                      {account.verifiedAt &&
                        ` · ${formatDateTime(account.verifiedAt)}`}
                      {account.verifyOnPublish && " · 发布时将验证"}
                      {account.cooldown
                        ? ` · ${cooldownLabels[account.cooldown.reason] ?? "来源冷却"}，至 ${formatDateTime(account.cooldown.untilMs)}`
                        : account.cooldownKnown
                          ? ""
                          : " · 冷却状态未知"}
                      {account.recovery &&
                        ` · ${cooldownLabels[account.recovery.reason] ?? "来源冷却"}，${recoveryLabels[account.recovery.state]}`}
                      {account.quota &&
                        ` · ${quotaLabels[account.quota] ?? "额度状态未知"}`}
                    </p>
                  ) : (
                    <p className="text-fg-muted">
                      {accountStates[account.state] ?? "账号状态待核查"}
                    </p>
                  )}
                </li>
              ))}
            </ol>
          )}
          {model.child && (
            <div className="min-w-0 space-y-2 border-l border-border pl-3">
              <p className="break-all text-label text-fg-muted">
                {model.child.name} · 固定第 {model.child.revision} 版
                {model.child.models.length > 0 && " · 子服务按自身规则选择候选"}
              </p>
              <CandidateModels models={model.child.models} />
            </div>
          )}
        </li>
      ))}
    </ul>
  );
}

export function PublicationReview({
  draft,
  storeId,
  disabled,
}: {
  draft: ModelServiceDraft;
  storeId?: string;
  disabled: boolean;
}) {
  const [selected, setSelected] = useState("default");
  const [frozen, setFrozen] = useState<ServicePreviewRequest | null>(null);
  const options: { key: string; label: string; entry: ServicePreviewEntry }[] =
    [
      {
        key: "default",
        label: "默认入口 · 未命中规则",
        entry: { kind: "default" },
      },
      ...(draft.routingPolicy?.rules ?? []).map((rule) => ({
        key: `rule:${rule.id}`,
        label: `命中规则 · ${rule.name || "未命名规则"}`,
        entry: { kind: "rule" as const, ruleId: rule.id },
      })),
      ...draft.models.map((model) => ({
        key: `model:${model.id}`,
        label: `指定模型 · ${model.name || "未命名模型"}`,
        entry: { kind: "specified" as const, modelId: model.id },
      })),
    ];
  const option =
    options.find((option) => option.key === selected) ?? options[0];
  const input =
    storeId && draft.models.length
      ? { storeId, draft, entry: option.entry }
      : null;
  const current = !!input && JSON.stringify(input) === JSON.stringify(frozen);
  const query = useSharedQuery(servicePreviewResource, frozen, current);
  const preview = current && !query.loading && !query.error ? query.data : null;
  return (
    <section
      className="min-w-0 space-y-3 rounded-md border border-border p-3"
      aria-label="发布前路由预览"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-body font-medium">发布前路由预览</h3>
        <Button
          size="sm"
          variant="secondary"
          disabled={disabled || !input}
          loading={current && query.loading}
          onClick={() => {
            if (!input) return;
            if (current) void query.reload();
            else setFrozen(input);
          }}
        >
          {frozen ? "重新读取预览" : "读取预览"}
        </Button>
      </div>
      <p className="text-label text-fg-muted">
        按草稿能力和指定入口显示候选范围。实际选择还取决于客户端已安装的目录、请求条件和当时的状态。
      </p>
      {frozen && (
        <Field>
          <FieldLabel htmlFor="publication-review-entry">预览入口</FieldLabel>
          <Select
            value={option.key}
            disabled={disabled || !input}
            onValueChange={(value) => {
              const next = options.find((option) => option.key === value);
              if (!next || !storeId) return;
              setSelected(next.key);
              setFrozen({ storeId, draft, entry: next.entry });
            }}
          >
            <SelectTrigger
              id="publication-review-entry"
              aria-label="预览入口"
            />
            <SelectContent>
              {options.map((option) => (
                <SelectItem key={option.key} value={option.key}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
      )}
      {frozen && !current && (
        <p role="status" className="text-label text-fg-muted">
          草稿或入口已变化，请重新读取预览。
        </p>
      )}
      {current && query.error && (
        <p role="alert" className="text-label text-fg-muted">
          预览不可用 · {query.error}
        </p>
      )}
      {current && !query.error && !preview && (
        <p role="status" className="text-label text-fg-muted">
          正在核对草稿和来源状态…
        </p>
      )}
      {preview && (
        <>
          <p className="text-label text-fg-muted">
            核对时间 {formatDateTime(preview.checkedAt, "full")}
          </p>
          {preview.entryState !== "configured" && (
            <p role="status" className="text-label text-fg-muted">
              {preview.entryState === "ruleUnavailable"
                ? "该条件规则或其目标当前不可用。"
                : "所选入口尚未配置。"}
            </p>
          )}
          <CandidateModels models={preview.models} />
        </>
      )}
    </section>
  );
}
