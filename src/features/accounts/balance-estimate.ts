import type { ProviderUsage } from "@/features/providers/types";
import type { BillingAccount } from "./types";

export interface BalanceEstimate {
  reasons: string[];
  balance: { amount: number; currency: string } | null;
  sevenDaySpend: number | null;
  dailySpend: number | null;
  remainingDays: number | null;
}

const currency = (value: string) => /^[A-Z]{3}$/.test(value);
const count = (value: number) => Number.isSafeInteger(value) && value >= 0;
const amount = (value: number) =>
  Number.isFinite(value) && value >= 0 && value <= Number.MAX_SAFE_INTEGER;

export function isBalanceUsageScope(
  usage: ProviderUsage | null,
  providerId: string,
): boolean {
  return (
    !!usage &&
    usage.query.providerId === providerId &&
    usage.query.source === "gateway" &&
    usage.query.days === 7 &&
    usage.query.agent == null &&
    usage.query.model == null &&
    usage.query.cursor == null
  );
}

/** A connection's retained gateway costs are a consumption sample, never an account bill. */
export function estimateProviderBalance(
  account: BillingAccount,
  providerId: string,
  usage: ProviderUsage | null,
  now: number,
  directoryError: boolean,
  usageError: boolean,
): BalanceEstimate {
  const result: BalanceEstimate = {
    reasons: [],
    balance: null,
    sevenDaySpend: null,
    dailySpend: null,
    remainingDays: null,
  };
  const reasons = result.reasons;
  if (
    !providerId ||
    account.providerId !== providerId ||
    account.accountType !== "api"
  ) {
    reasons.push("尚未确认此连接的 API 计费账户。");
    return result;
  }
  const snapshot = account.snapshot;
  if (!snapshot) reasons.push("尚未读取余额快照，请先读取真实账户。");
  else {
    if (snapshot.accountType !== "api")
      reasons.push("余额快照的账户类型不一致。");
    const observed = Date.parse(snapshot.asOf);
    if (directoryError || account.error || account.state === "refresh_failed")
      reasons.push("余额读取失败，暂停估算。");
    else if (
      account.state === "stale" ||
      (Number.isFinite(observed) &&
        Number.isFinite(snapshot.validForSeconds) &&
        snapshot.validForSeconds >= 0 &&
        now - observed > snapshot.validForSeconds * 1000)
    )
      reasons.push("余额快照已过期，请刷新账户用量。");
    else if (account.state !== "ready")
      reasons.push("余额来源尚未就绪，暂停估算。");
    if (
      !Number.isFinite(now) ||
      !Number.isFinite(observed) ||
      observed > now ||
      !Number.isFinite(snapshot.validForSeconds) ||
      snapshot.validForSeconds < 0
    )
      reasons.push("余额采集时间或有效期未知。");
    const balances = snapshot.money.filter((entry) => entry.kind === "balance");
    const wallet = balances.length === 1 ? balances[0] : null;
    if (
      !wallet ||
      wallet.amount === null ||
      !/^(?:0|[1-9]\d*)(?:\.\d+)?$/.test(wallet.amount.trim()) ||
      !amount(Number(wallet.amount)) ||
      !currency(wallet.currency)
    )
      reasons.push("未取得单一且可确认币种的有效余额。");
    else
      result.balance = {
        amount: Number(wallet.amount),
        currency: wallet.currency,
      };
  }
  if (!usage) {
    reasons.push(
      usageError ? "近 7 天调用费用读取失败。" : "尚未取得近 7 天调用费用。",
    );
    return result;
  }
  if (usageError) reasons.push("调用费用未更新，暂停估算。");
  if (!isBalanceUsageScope(usage, providerId)) {
    reasons.push("调用统计范围与当前连接的近 7 天范围不一致。");
    return result;
  }
  if (usage.sourceState !== "ready")
    reasons.push("调用来源未就绪或仅部分采集。");
  const statsAt = Date.parse(usage.asOf),
    sourceAt = Date.parse(usage.sourceAsOf);
  if (
    ![statsAt, sourceAt].every(
      (time) => Number.isFinite(time) && time <= now && now - time <= 30_000,
    )
  )
    reasons.push("调用费用快照已过期或时间未知，请重新读取调用费用。");
  const totals = usage.selected;
  if (!totals) {
    reasons.push("此连接暂无可确认的费用汇总。");
    return result;
  }
  const buckets = [...totals.cost.actual, ...totals.cost.estimated];
  if (
    !buckets.every(
      (entry) =>
        currency(entry.currency) &&
        amount(entry.amount) &&
        count(entry.calls) &&
        entry.calls > 0,
    )
  ) {
    reasons.push("调用费用金额、币种或样本数量无效。");
    return result;
  }
  const currencies = [...new Set(buckets.map((entry) => entry.currency))];
  const balanceCurrency = result.balance?.currency;
  if (balanceCurrency && currencies.some((value) => value !== balanceCurrency))
    reasons.push(
      `币种不一致：余额为 ${balanceCurrency}，调用费用为 ${currencies.join(" / ")}，不自动换汇。`,
    );
  const knownCalls = buckets.reduce((sum, entry) => sum + entry.calls, 0);
  if (
    !count(totals.calls) ||
    !count(totals.cost.unknownCalls) ||
    knownCalls + totals.cost.unknownCalls !== totals.calls
  )
    reasons.push("费用覆盖数量与调用总数不一致。");
  if (totals.cost.unknownCalls > 0)
    reasons.push(
      `${totals.cost.unknownCalls.toLocaleString("zh-CN")} 次调用费用未知，不能用已知部分推算剩余时长。`,
    );
  if (!count(totals.inProgress) || totals.inProgress > 0)
    reasons.push("仍有进行中或状态不明的调用。");
  if (!count(usage.missingTime) || usage.missingTime > 0)
    reasons.push("存在时间不明的调用，近 7 天范围不完整。");
  if (knownCalls < 2)
    reasons.push("费用样本不足，至少需要 2 次费用明确的调用。");
  const spend = buckets.reduce((sum, entry) => sum + entry.amount, 0);
  if (!amount(spend) || spend <= 0)
    reasons.push("近 7 天费用为零或无有效金额，消耗速度未知。");
  if (!result.balance || reasons.length) return result;
  const dailySpend = spend / 7;
  const remainingDays = result.balance.amount / dailySpend;
  if (
    !Number.isFinite(remainingDays) ||
    !Number.isSafeInteger(Math.ceil(remainingDays))
  ) {
    reasons.push("消耗速度过低，剩余时长超出可可靠表达的范围。");
    return result;
  }
  return { ...result, sevenDaySpend: spend, dailySpend, remainingDays };
}

export function balanceDuration(days: number | null): string {
  if (days === null) return "未知";
  if (days === 0) return "余额已用完";
  if (days < 1 / 24) return "不足 1 小时";
  return days < 1
    ? `约 ${(days * 24).toLocaleString("zh-CN", { maximumFractionDigits: 1 })} 小时`
    : `约 ${days.toLocaleString("zh-CN", { maximumFractionDigits: 1 })} 天`;
}
