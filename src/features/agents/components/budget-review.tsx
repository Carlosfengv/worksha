import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import {
  localConnectionsApi,
  type BudgetAttempt,
  type BudgetReview,
} from "../api";

const names = { session: "会话", daily: "每日", monthly: "每月" };
type Mode = "skip" | "settle" | "release";
type Metric = { mode: Mode; amount: string };

function metric(value: Metric, usd: boolean): BudgetReview["tokens"] {
  if (value.mode === "skip") return null;
  if (value.mode === "release") return { kind: "release" };
  const amount = Number(value.amount) * (usd ? 1_000_000 : 1);
  if (
    !value.amount.trim() ||
    !Number.isSafeInteger(Math.round(amount)) ||
    amount < 0
  )
    throw Error("请输入可核实的非负用量");
  if (Math.abs(amount - Math.round(amount)) > 1e-7)
    throw Error("Token 需为整数，USD 至多 6 位小数");
  return { kind: "settle", amount: Math.round(amount) };
}

function MetricChoice({
  label,
  value,
  onChange,
}: {
  label: string;
  value: Metric;
  onChange: (value: Metric) => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Select
        value={value.mode}
        onValueChange={(mode) => {
          if (mode === "skip" || mode === "settle" || mode === "release")
            onChange({ ...value, mode });
        }}
      >
        <SelectTrigger aria-label={`${label}核查方式`} />
        <SelectContent>
          <SelectItem value="skip">继续冻结</SelectItem>
          <SelectItem value="settle">按实际结算</SelectItem>
          <SelectItem value="release">确认零用量并释放</SelectItem>
        </SelectContent>
      </Select>
      {value.mode === "settle" && (
        <Input
          type="number"
          min="0"
          step="any"
          aria-label={`${label}实际用量`}
          value={value.amount}
          onChange={(event) =>
            onChange({ ...value, amount: event.target.value })
          }
        />
      )}
    </div>
  );
}

export function BudgetReviewControls({
  attempt,
  storeId,
  onApplied,
}: {
  attempt: BudgetAttempt;
  storeId: string;
  onApplied: () => void;
}) {
  const [upstreamRequestId, setUpstream] = useState(
    attempt.upstreamRequestId ?? "",
  );
  const [source, setSource] = useState("");
  const [tokens, setTokens] = useState<Metric>({ mode: "skip", amount: "" });
  const [usd, setUsd] = useState<Metric>({ mode: "skip", amount: "" });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [pending, setPending] = useState<BudgetReview | null>(null);
  const review = async () => {
    setBusy(true);
    setError("");
    try {
      const input = pending ?? {
        storeId,
        operationId: crypto.randomUUID(),
        bindingId: attempt.bindingId,
        attemptKey: attempt.attemptKey,
        expectedRevision: attempt.revision,
        upstreamRequestId: upstreamRequestId.trim(),
        source: source.trim(),
        tokens: metric(tokens, false),
        usdMicros: metric(usd, true),
      };
      setPending(input);
      await localConnectionsApi.review(input);
      setPending(null);
      onApplied();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section aria-label="待核查预算" className="space-y-3">
      <p>
        {attempt.providerId} / {attempt.model} ·{" "}
        {new Date(attempt.createdAt).toLocaleString("zh-CN")}
      </p>
      <p className="text-label text-fg-muted">
        {attempt.tier === "primary" ? "主池" : "备用池"} ·{" "}
        {attempt.windows
          .map(({ kind, key }) => `${names[kind]} ${key}`)
          .join(" · ")}
      </p>
      <a
        className="underline"
        href={`#/requests/${encodeURIComponent(attempt.observationId)}`}
      >
        查看此请求
      </a>
      <fieldset disabled={busy || pending !== null} className="space-y-2">
        <label className="block">
          上游请求标识
          <Input
            value={upstreamRequestId}
            maxLength={200}
            onChange={(e) => setUpstream(e.target.value)}
            readOnly={attempt.upstreamRequestId !== null}
          />
        </label>
        <label className="block">
          证据出处
          <Input
            value={source}
            maxLength={320}
            onChange={(e) => setSource(e.target.value)}
          />
        </label>
        {attempt.frozen.tokens !== null && (
          <div>
            <p>冻结 {attempt.frozen.tokens} Tokens</p>
            <MetricChoice label="Token" value={tokens} onChange={setTokens} />
          </div>
        )}
        {attempt.frozen.usdMicros !== null && (
          <div>
            <p>冻结 USD {attempt.frozen.usdMicros / 1_000_000}</p>
            <MetricChoice label="USD" value={usd} onChange={setUsd} />
          </div>
        )}
      </fieldset>
      <div className="flex flex-wrap gap-2">
        <Button
          disabled={
            busy ||
            !upstreamRequestId.trim() ||
            !source.trim() ||
            (tokens.mode === "skip" && usd.mode === "skip")
          }
          onClick={() => void review()}
        >
          {pending ? "核对原操作" : "提交核查"}
        </Button>
        {pending && (
          <Button
            variant="tertiary"
            disabled={busy}
            onClick={() => {
              setPending(null);
              onApplied();
            }}
          >
            重新读取核查状态
          </Button>
        )}
      </div>
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
