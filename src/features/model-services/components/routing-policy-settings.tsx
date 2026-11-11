import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import {
  defaultRoutingPolicy,
  type ModelServiceDraft,
  type RoutingPolicy,
} from "../types";
import { RouteRulesEditor } from "./route-rules-editor";
import { IntentClassifierSettings } from "./intent-classifier-settings";

export function RoutingPolicySettings({
  value,
  models,
  storeId,
  disabled,
  onChange,
}: {
  value?: RoutingPolicy;
  models: ModelServiceDraft["models"];
  storeId?: string;
  disabled: boolean;
  onChange: (policy: RoutingPolicy) => void;
}) {
  const policy = value ?? defaultRoutingPolicy();
  const update = (patch: Partial<RoutingPolicy>) =>
    onChange({ ...policy, ...patch });
  return (
    <section className="space-y-4" aria-label="路由策略">
      <Switch
        label="启用优先级与自动回退"
        checked={policy.enabled}
        disabled={
          disabled ||
          !!policy.accountPools?.length ||
          models.some((m) => m.child)
        }
        onCheckedChange={(enabled) => update({ enabled })}
      />
      <p className="text-label text-fg-muted">
        默认入口先检查条件规则，再按优先级选择。指定模型优先尝试所选条目。
      </p>
      {policy.enabled && (
        <>
          <div className="flex flex-wrap gap-4">
            <Switch
              label="上游临时故障时回退"
              checked={policy.fallbackOnFailure}
              disabled={disabled}
              onCheckedChange={(fallbackOnFailure) =>
                update({ fallbackOnFailure })
              }
            />
            <Switch
              label="上游或配置限流时切换"
              checked={policy.fallbackOnRateLimit}
              disabled={disabled}
              onCheckedChange={(fallbackOnRateLimit) =>
                update({ fallbackOnRateLimit })
              }
            />
            <Switch
              label="上游明确拒绝凭据或模型权限时回退"
              checked={policy.fallbackOnAccessDenied === true}
              disabled={disabled}
              onCheckedChange={(fallbackOnAccessDenied) =>
                update({ fallbackOnAccessDenied })
              }
            />
            {!!policy.accountPools?.length && (
              <Switch
                label="账号池耗尽后允许跨模型回退"
                checked={policy.fallbackAcrossModels === true}
                disabled={disabled}
                onCheckedChange={(fallbackAcrossModels) =>
                  update({ fallbackAcrossModels })
                }
              />
            )}
          </div>
          <div className="grid gap-4 md:grid-cols-3">
            {(
              [
                ["maxAttempts", "最多尝试候选", 1, 5],
                ["deadlineSeconds", "总等待时间（秒）", 5, 300],
                ["cooldownSeconds", "默认冷却（秒）", 1, 3600],
              ] as const
            ).map(([key, label, min, max]) => (
              <Field key={key}>
                <FieldLabel htmlFor={`policy-${key}`}>{label}</FieldLabel>
                <Input
                  id={`policy-${key}`}
                  type="number"
                  min={min}
                  max={max}
                  value={policy[key]}
                  disabled={disabled}
                  onChange={(e) => update({ [key]: Number(e.target.value) })}
                />
              </Field>
            ))}
          </div>
          <p className="text-label text-fg-muted">
            仅在模型输出提交前回退；工具续调、签名历史和已开始的回复保留原目标。429
            优先遵守上游冷却时间，同一凭据共享冷却状态。
          </p>
        </>
      )}
      {(policy.enabled || policy.intentClassifier) && (
        <IntentClassifierSettings
          policy={policy}
          models={models}
          storeId={storeId}
          disabled={disabled}
          onChange={(intentClassifier) => update({ intentClassifier })}
        />
      )}
      {(policy.enabled || !!policy.rules?.length) && (
        <RouteRulesEditor
          rules={policy.rules ?? []}
          models={models}
          disabledModels={policy.disabledModels}
          disabled={disabled}
          onChange={(rules) => update({ rules })}
        />
      )}
    </section>
  );
}
