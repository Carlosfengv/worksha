import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import { createReadResourceFamily } from "@/shared/lib/read-resource";
import type { ModelService } from "@/features/model-services/types";
import {
  localConnectionsApi,
  type BudgetAmounts,
  type BudgetPolicy,
  type BudgetWindow,
} from "../api";
import { BudgetReviewControls } from "./budget-review";

const resource = createReadResourceFamily(localConnectionsApi.budget);
const names = { session: "会话", daily: "每日", monthly: "每月" };
const kinds: BudgetWindow[] = ["session", "daily", "monthly"];

function BudgetEditor({
  bindingId,
  policy,
  storeId,
  services,
  onApplied,
}: {
  bindingId: string;
  policy: BudgetPolicy | null;
  storeId: string;
  services: ModelService[];
  onApplied: () => void;
}) {
  const [draft, setDraft] = useState<BudgetPolicy>(
    policy ?? {
      revision: 0,
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
      fallbackServiceId: null,
      warnPercent: 80,
      windows: [],
    },
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const change = (
    kind: BudgetWindow,
    tier: "primary" | "fallback",
    metric: keyof BudgetAmounts,
    raw: string,
  ) => {
    const amount =
      raw === ""
        ? null
        : Math.round(Number(raw) * (metric === "usdMicros" ? 1_000_000 : 1));
    setDraft((current) => {
      const row = current.windows.find((row) => row.kind === kind) ?? {
        kind,
        primary: { tokens: null, usdMicros: null },
        fallback: { tokens: null, usdMicros: null },
      };
      return {
        ...current,
        windows: [
          ...current.windows.filter((row) => row.kind !== kind),
          {
            ...row,
            [tier]: { ...row[tier], [metric]: amount },
          },
        ],
      };
    });
  };
  const save = async () => {
    setBusy(true);
    setError("");
    try {
      const windows = draft.windows
        .filter(
          (row) =>
            row.primary.tokens !== null || row.primary.usdMicros !== null,
        )
        .map((row) =>
          draft.fallbackServiceId
            ? row
            : { ...row, fallback: { tokens: null, usdMicros: null } },
        );
      await localConnectionsApi.saveBudget(bindingId, storeId, {
        ...draft,
        windows,
      });
      onApplied();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section aria-label="预算设置" className="space-y-3">
      <fieldset disabled={busy} className="space-y-3">
        <label className="block">
          计费时区
          <Input
            value={draft.timezone}
            readOnly={policy !== null}
            onChange={(e) => setDraft({ ...draft, timezone: e.target.value })}
          />
        </label>
        <label className="block">
          预警百分比
          <Input
            type="number"
            min="1"
            max="100"
            value={draft.warnPercent}
            onChange={(e) =>
              setDraft({ ...draft, warnPercent: Number(e.target.value) })
            }
          />
        </label>
        <Select
          value={draft.fallbackServiceId ?? "stop"}
          onValueChange={(value) =>
            setDraft({
              ...draft,
              fallbackServiceId: value === "stop" ? null : value,
            })
          }
        >
          <SelectTrigger aria-label="超额处理" />
          <SelectContent>
            <SelectItem value="stop">超额拦截</SelectItem>
            {services.map((service) => (
              <SelectItem key={service.id} value={service.id}>
                降级至 {service.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <p className="text-label text-fg-muted">
          留空表示该指标没有限额。会话限额需要客户端提供有效会话 ID。
        </p>
        {kinds.map((kind) => (
          <fieldset key={kind} className="space-y-2">
            <legend className="text-label font-medium">{names[kind]}</legend>
            <div className="grid gap-2 sm:grid-cols-2">
              {(["primary", "fallback"] as const)
                .filter((tier) => tier === "primary" || draft.fallbackServiceId)
                .map((tier) =>
                  (["tokens", "usdMicros"] as const).map((metric) => {
                    const limit = draft.windows.find(
                      (row) => row.kind === kind,
                    )?.[tier][metric];
                    const label = `${names[kind]}${tier === "primary" ? "主池" : "备用池"} ${metric === "tokens" ? "Token" : "USD"} 上限`;
                    return (
                      <label key={`${tier}-${metric}`}>
                        {label}
                        <Input
                          type="number"
                          min="0"
                          step={metric === "tokens" ? 1 : 0.000001}
                          value={
                            limit == null
                              ? ""
                              : metric === "usdMicros"
                                ? limit / 1_000_000
                                : limit
                          }
                          aria-label={label}
                          onChange={(event) =>
                            change(kind, tier, metric, event.target.value)
                          }
                        />
                      </label>
                    );
                  }),
                )}
            </div>
          </fieldset>
        ))}
      </fieldset>
      <Button disabled={busy} onClick={() => void save()}>
        保存预算
      </Button>
      {error && <p role="alert">{error}</p>}
    </section>
  );
}

export function BindingBudget({
  bindingId,
  services,
  revoked,
}: {
  bindingId: string;
  services: ModelService[];
  revoked: boolean;
}) {
  const data = useSharedQuery(resource, bindingId);
  const [selected, setSelected] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const attempt = data.data?.attempts.find(
    (item) => item.attemptKey === selected,
  );
  if (data.error) return <p role="alert">{data.error}</p>;
  if (!data.data) return <p role="status">正在读取预算…</p>;
  const { policy, storeId, windows, attempts } = data.data;
  return (
    <section aria-label="绑定预算" className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-heading-sm font-medium">绑定预算</h3>
        {!revoked && (
          <Button variant="tertiary" onClick={() => setEditing(!editing)}>
            {editing ? "关闭预算设置" : "编辑预算"}
          </Button>
        )}
        <Button variant="tertiary" onClick={() => void data.invalidate()}>
          刷新预算
        </Button>
      </div>
      {!policy && (
        <p className="text-label text-fg-muted">此绑定未配置预算。</p>
      )}
      {windows
        .filter((window) => window.warned)
        .map((window) => (
          <p key={`${window.tier}-${window.kind}-${window.key}`} role="status">
            {window.tier === "primary" ? "主池" : "备用池"} ·{" "}
            {names[window.kind]} {window.key} 已达到预警阈值
          </p>
        ))}
      {editing && (
        <BudgetEditor
          key={policy?.revision ?? 0}
          bindingId={bindingId}
          policy={policy}
          storeId={storeId}
          services={services}
          onApplied={() => {
            setEditing(false);
            void data.invalidate();
          }}
        />
      )}
      {attempts.length > 0 && (
        <Select value={selected ?? ""} onValueChange={setSelected}>
          <SelectTrigger aria-label="选择待核查预算尝试" />
          <SelectContent>
            {attempts.map((item) => (
              <SelectItem key={item.attemptKey} value={item.attemptKey}>
                {item.model} ·{" "}
                {new Date(item.createdAt).toLocaleString("zh-CN")} ·{" "}
                {item.attemptKey.slice(0, 8)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      {attempt && (
        <BudgetReviewControls
          key={`${attempt.attemptKey}:${attempt.revision}`}
          attempt={attempt}
          storeId={storeId}
          onApplied={() => {
            setSelected(null);
            void data.invalidate();
          }}
        />
      )}
    </section>
  );
}
