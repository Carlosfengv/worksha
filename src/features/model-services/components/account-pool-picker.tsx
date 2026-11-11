import { Button } from "@/components/ui/button";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { upstreamAccountsResource } from "@/features/providers/queries";
import type { UpstreamAccountRef } from "@/features/providers/types";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import { accountStrategies, type AccountPool } from "../types";

const strategies = {
  priority: "固定顺序",
  "round-robin": "依次轮换",
  weighted: "按权重分配",
  "quota-reset": "优先用即将重置的额度",
  "quota-remaining": "优先用剩余额度比例高的账号",
  "quota-pace": "优先用每小时剩余额度比例高的账号",
} as const;
const affinities = {
  off: "关闭",
  prefer: "优先沿用",
  strict: "固定账号",
} as const;

export function AccountPoolPicker({
  modelId,
  providerId,
  storeId,
  pool,
  onChange,
  disabled,
}: {
  modelId: string;
  providerId: string;
  storeId?: string;
  pool?: AccountPool;
  onChange: (pool?: AccountPool) => void;
  disabled: boolean;
}) {
  const value = pool?.accounts ?? [];
  const setAccounts = (accounts: UpstreamAccountRef[]) => {
    if (!accounts.length) {
      onChange();
      return;
    }
    const weights = Object.fromEntries(
      Object.entries(pool?.weights ?? {}).filter(([id]) =>
        accounts.some((account) => account.id === id),
      ),
    );
    onChange({ ...pool, modelId, accounts, weights });
  };
  const query = useSharedQuery(upstreamAccountsResource, providerId);
  const accounts =
    query.data?.storeId === storeId ? query.data?.accounts : undefined;
  const available = (accounts ?? []).filter(
    (a) => a.state === "enabled" && !value.some((ref) => ref.id === a.id),
  );
  return (
    <fieldset className="space-y-2" disabled={disabled}>
      <legend className="text-body font-medium">上游账号池</legend>
      <p className="text-label text-fg-muted">
        为同一模型选择可用账号。留空时使用 Provider 的默认连接。
      </p>
      {query.error || (query.data && !accounts) ? (
        <p role="alert" className="text-body text-fg-muted">
          上游账号未能读取。请刷新原后台后核查。
        </p>
      ) : query.loading ? (
        <p role="status" className="text-label text-fg-muted">
          正在读取上游账号…
        </p>
      ) : null}
      {pool && (
        <>
          <div className="grid gap-3 md:grid-cols-2">
            <Field>
              <FieldLabel htmlFor="account-strategy">账号选择策略</FieldLabel>
              <Select
                value={pool.strategy ?? "priority"}
                disabled={disabled}
                onValueChange={(value) => {
                  const strategy = accountStrategies.find((s) => s === value);
                  if (strategy)
                    onChange({
                      ...pool,
                      strategy,
                      weights:
                        strategy === "weighted" ? pool.weights : undefined,
                    });
                }}
              >
                <SelectTrigger id="account-strategy" />
                <SelectContent>
                  {Object.entries(strategies).map(([value, label]) => (
                    <SelectItem key={value} value={value}>
                      {label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
            <Field>
              <FieldLabel htmlFor="account-affinity">同会话后续轮次</FieldLabel>
              <Select
                value={pool.affinity ?? "off"}
                disabled={disabled}
                onValueChange={(value) => {
                  if (
                    value === "off" ||
                    value === "prefer" ||
                    value === "strict"
                  )
                    onChange({ ...pool, affinity: value });
                }}
              >
                <SelectTrigger id="account-affinity" />
                <SelectContent>
                  {Object.entries(affinities).map(([value, label]) => (
                    <SelectItem key={value} value={value}>
                      {label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
          </div>
          {pool.strategy?.startsWith("quota-") && (
            <p className="text-label text-fg-muted">
              额度排序当前使用 OpenRouter 的密钥预算或 MiniMax M Plan
              的订阅内额度。在供应商的账户额度中刷新每个账号。额度未知时按配置顺序尝试。
              {pool.strategy === "quota-pace" &&
                "每小时剩余额度比例按到重置前的剩余时间折算。"}
            </p>
          )}
          {pool.affinity === "strict" && (
            <p className="text-label text-fg-muted">
              同一模型成功输出后固定该账号。账号不可用时，本次请求会停止。
            </p>
          )}
        </>
      )}
      {value.length > 0 && (
        <ol className="space-y-2">
          {value.map((ref, index) => {
            const account = accounts?.find((a) => a.id === ref.id);
            const valid =
              account?.revision === ref.revision && account.state === "enabled";
            const name = account?.label ?? `账号 ${index + 1}`;
            return (
              <li key={ref.id} className="flex flex-wrap items-center gap-2">
                <span className="min-w-0 break-words text-body">
                  {index + 1}. {name}
                  {!valid ? " · 待核查" : ""}
                </span>
                {pool?.strategy === "weighted" && (
                  <Input
                    type="number"
                    className="w-24"
                    size="sm"
                    aria-label={`${name} 权重`}
                    placeholder="权重"
                    min={1}
                    max={1000}
                    step={1}
                    disabled={disabled}
                    value={
                      pool.weights?.[ref.id] === 0
                        ? ""
                        : (pool.weights?.[ref.id] ?? 1)
                    }
                    onChange={(e) =>
                      onChange({
                        ...pool,
                        weights: {
                          ...pool.weights,
                          [ref.id]: Number(e.target.value),
                        },
                      })
                    }
                  />
                )}
                <Button
                  variant="tertiary"
                  size="sm"
                  disabled={disabled || index === 0}
                  aria-label={`上移 ${name}`}
                  onClick={() => {
                    const next = [...value];
                    [next[index - 1], next[index]] = [
                      next[index],
                      next[index - 1],
                    ];
                    setAccounts(next);
                  }}
                >
                  上移
                </Button>
                <Button
                  variant="tertiary"
                  size="sm"
                  disabled={disabled}
                  aria-label={`移除 ${name}`}
                  onClick={() =>
                    setAccounts(value.filter((a) => a.id !== ref.id))
                  }
                >
                  移除
                </Button>
                {account?.state === "enabled" &&
                  account.revision !== ref.revision && (
                    <Button
                      variant="tertiary"
                      size="sm"
                      disabled={disabled}
                      onClick={() =>
                        setAccounts(
                          value.map((a) =>
                            a.id === ref.id
                              ? { id: account.id, revision: account.revision }
                              : a,
                          ),
                        )
                      }
                    >
                      采用新版
                    </Button>
                  )}
              </li>
            );
          })}
        </ol>
      )}
      {available.length > 0 && value.length < 8 && (
        <Combobox<string>
          items={available.map((a) => a.id)}
          value={null}
          disabled={disabled}
          itemToStringLabel={(id) =>
            available.find((a) => a.id === id)?.label ?? id
          }
          onValueChange={(id) => {
            const account = available.find((a) => a.id === id);
            if (account)
              setAccounts([
                ...value,
                { id: account.id, revision: account.revision },
              ]);
          }}
        >
          <ComboboxInput
            aria-label="添加上游账号"
            placeholder="搜索并添加上游账号"
          />
          <ComboboxContent>
            <ComboboxEmpty>没有匹配账号</ComboboxEmpty>
            <ComboboxList>
              {(id) => (
                <ComboboxItem key={id} value={id}>
                  {available.find((a) => a.id === id)?.label}
                </ComboboxItem>
              )}
            </ComboboxList>
          </ComboboxContent>
        </Combobox>
      )}
      {accounts?.length === 0 && (
        <p className="text-label text-fg-muted">
          可以在供应商详情的「上游账号」中添加账号。
        </p>
      )}
    </fieldset>
  );
}
