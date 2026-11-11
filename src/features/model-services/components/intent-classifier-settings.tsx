import { Field, FieldLabel } from "@/components/ui/field";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { upstreamAccountsResource } from "@/features/providers/queries";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import type {
  ChildServiceRef,
  IntentClassifier,
  RoutingPolicy,
} from "../types";

export function classifierReady(
  policy: RoutingPolicy,
  models: { id: string; child?: ChildServiceRef }[],
) {
  const classifier = policy.intentClassifier;
  if (!classifier) return true;
  const accounts =
    policy.accountPools?.find((pool) => pool.modelId === classifier.modelId)
      ?.accounts ?? [];
  return (
    models.some((model) => model.id === classifier.modelId && !model.child) &&
    !policy.disabledModels.includes(classifier.modelId) &&
    (classifier.account
      ? accounts.some(
          (account) =>
            account.id === classifier.account?.id &&
            account.revision === classifier.account.revision,
        )
      : accounts.length === 0)
  );
}

export function IntentClassifierSettings({
  policy,
  models,
  storeId,
  disabled,
  onChange,
}: {
  policy: RoutingPolicy;
  models: {
    id: string;
    name: string;
    providerId: string;
    child?: ChildServiceRef;
  }[];
  storeId?: string;
  disabled: boolean;
  onChange: (value?: IntentClassifier) => void;
}) {
  const classifier = policy.intentClassifier;
  const enabled = models.filter(
    (model) => !model.child && !policy.disabledModels.includes(model.id),
  );
  const selected = enabled.find((model) => model.id === classifier?.modelId);
  const accounts =
    policy.accountPools?.find((pool) => pool.modelId === selected?.id)
      ?.accounts ?? [];
  const query = useSharedQuery(
    upstreamAccountsResource,
    selected?.providerId ?? "",
    !!selected?.providerId && accounts.length > 0,
  );
  const labels =
    query.data?.storeId === storeId ? query.data?.accounts : undefined;
  return (
    <fieldset className="space-y-3" disabled={disabled}>
      <legend className="text-body font-medium">意图路由</legend>
      <p className="text-label text-fg-muted">
        启用后，可能命中意图规则时，会向所选模型发送最新用户文字的过滤片段。分类请求可能产生费用。
      </p>
      <Switch
        label="启用意图分类"
        checked={!!classifier}
        disabled={disabled || (!enabled.length && !classifier)}
        onCheckedChange={(checked) =>
          onChange(checked ? { modelId: "" } : undefined)
        }
      />
      {classifier && (
        <>
          <div className="grid gap-3 md:grid-cols-2">
            <Field>
              <FieldLabel>分类模型</FieldLabel>
              <Select
                value={selected?.id ?? ""}
                disabled={disabled}
                onValueChange={(modelId) => onChange({ modelId })}
              >
                <SelectTrigger
                  aria-label="分类模型"
                  placeholder="选择已启用的服务模型"
                />
                <SelectContent>
                  {enabled.map((model) => (
                    <SelectItem key={model.id} value={model.id}>
                      {model.name || "未命名模型"}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
            {!!accounts.length && (
              <Field>
                <FieldLabel>分类账号</FieldLabel>
                <Select
                  value={
                    accounts.find(
                      (account) =>
                        account.id === classifier.account?.id &&
                        account.revision === classifier.account.revision,
                    )?.id ?? ""
                  }
                  disabled={disabled}
                  onValueChange={(id) =>
                    onChange({
                      modelId: classifier.modelId,
                      account: accounts.find((account) => account.id === id),
                    })
                  }
                >
                  <SelectTrigger
                    aria-label="分类账号"
                    placeholder="选择固定账号"
                  />
                  <SelectContent>
                    {accounts.map((account, index) => (
                      <SelectItem key={account.id} value={account.id}>
                        {labels?.find((label) => label.id === account.id)
                          ?.label ?? `账号 ${index + 1}`}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </Field>
            )}
          </div>
          {!classifierReady(policy, models) && (
            <p className="text-label text-fg-muted" role="status">
              请选择启用的分类模型。有账号池时还需选定一个账号。
            </p>
          )}
        </>
      )}
    </fieldset>
  );
}
