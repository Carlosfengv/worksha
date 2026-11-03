import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { defaultRoutingPolicy, type RoutingPolicy } from "../types";

export function RoutingPolicySettings({
  value,
  disabled,
  onChange,
}: {
  value?: RoutingPolicy;
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
        disabled={disabled}
        onCheckedChange={(enabled) => update({ enabled })}
      />
      <p className="text-label text-fg-muted">
        默认按优先级选择；指定模型先用所选条目，触发策略后再回退，客户端选择保持。
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
    </section>
  );
}
