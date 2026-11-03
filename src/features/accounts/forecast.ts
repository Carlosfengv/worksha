import type { AccountSnapshot, BillingAccount } from "./types";

type QuotaWindow = AccountSnapshot["windows"][number];
export type QuotaForecast = {
  id: string;
  label: string;
  usedPercent: number | null;
  remainingPercent: number | null;
  resetsAt: number | null;
  observedAt: number | null;
  state: "ready" | "unknown" | "stale" | "insufficient";
  reason: string;
  projectedPercent: number | null;
  runsOutAt: number | null;
};

function percent(value: number | null): number | null {
  return value !== null && Number.isFinite(value) && value >= 0 && value <= 100
    ? value
    : null;
}

function timestamp(value: number | null): number | null {
  return value !== null &&
    Number.isFinite(value) &&
    value > 0 &&
    Number.isFinite(new Date(value).getTime())
    ? value
    : null;
}

/** Only an explicitly linked account can supply Provider quota observations. */
export function providerQuotaForecasts(
  account: BillingAccount,
  providerId: string,
  now: number,
  directoryError: boolean,
): QuotaForecast[] {
  if (!providerId || account.providerId !== providerId || !account.snapshot)
    return [];
  return accountQuotaForecasts(account, now, directoryError);
}

/** The caller selects one explicit account; native subscriptions need no API link. */
export function accountQuotaForecasts(
  account: BillingAccount,
  now: number,
  directoryError: boolean,
): QuotaForecast[] {
  if (!account.snapshot) return [];
  const snapshot = account.snapshot;
  const observedAt = timestamp(Date.parse(snapshot.asOf));
  return snapshot.windows.map((window) => {
    const row: QuotaForecast = {
      id: JSON.stringify([account.id, window.id]),
      label: window.label,
      usedPercent: percent(window.usedPercent),
      remainingPercent: percent(window.remainingPercent),
      resetsAt: timestamp(
        window.resetsAt === null ? null : window.resetsAt * 1000,
      ),
      observedAt,
      state: "unknown",
      reason: "来源未提供完整额度数据。",
      projectedPercent: null,
      runsOutAt: null,
    };
    if (directoryError || account.state === "refresh_failed" || account.error) {
      row.state = "stale";
      row.reason = "读取未成功，保留上次快照，暂停预测。";
    } else if (
      account.state === "stale" ||
      (observedAt !== null &&
        Number.isFinite(snapshot.validForSeconds) &&
        snapshot.validForSeconds >= 0 &&
        now - observedAt > snapshot.validForSeconds * 1000)
    ) {
      row.state = "stale";
      row.reason = "快照已过期，刷新账户用量后再预测。";
    } else if (account.state !== "ready") {
      row.reason =
        account.state === "refreshing"
          ? "账户用量正在刷新，暂不更新预测。"
          : "账户来源尚未就绪，额度预测保持未知。";
    } else if (
      !Number.isFinite(now) ||
      observedAt === null ||
      observedAt > now ||
      !Number.isFinite(snapshot.validForSeconds) ||
      snapshot.validForSeconds < 0
    ) {
      row.reason = "快照时间或有效期未知，暂无法预测。";
    } else if (row.usedPercent === null) {
      row.reason = "来源未提供已用百分比，暂无法预测。";
    } else if (row.resetsAt === null) {
      row.reason = "来源未提供重置时间，暂无法预测。";
    } else if (row.resetsAt <= now) {
      row.state = "stale";
      row.reason = "已到重置时间，等待新的额度快照。";
    } else {
      projectWindow(row, window);
    }
    return row;
  });
}

function projectWindow(row: QuotaForecast, window: QuotaWindow) {
  const { observedAt, resetsAt, usedPercent } = row;
  if (observedAt === null || resetsAt === null || usedPercent === null) return;
  const duration = window.durationMins;
  if (duration === null || !Number.isFinite(duration) || duration <= 0) {
    row.state = "insufficient";
    row.reason = "来源未提供窗口时长，数据不足，暂无法预测。";
    return;
  }
  const start = resetsAt - duration * 60_000;
  const elapsed = observedAt - start;
  if (!Number.isFinite(start) || elapsed <= 0) {
    row.state = "insufficient";
    row.reason = "当前窗口刚开始或周期不完整，数据不足，暂无法预测。";
    return;
  }
  const rate = usedPercent / elapsed;
  const projected = usedPercent + rate * (resetsAt - observedAt);
  if (!Number.isFinite(projected)) {
    row.state = "insufficient";
    row.reason = "当前周期数据不足，暂无法预测。";
    return;
  }
  row.state = "ready";
  row.projectedPercent = projected;
  row.runsOutAt =
    usedPercent === 100
      ? observedAt
      : projected > 100
        ? observedAt + (100 - usedPercent) / rate
        : null;
  row.reason = "按快照时的周期平均消耗速度线性估算。";
}

export function quotaPercent(value: number | null): string {
  return value === null ? "未知" : `${Number(value.toFixed(1))}%`;
}

export function quotaOutcome(row: QuotaForecast): string {
  if (row.state === "stale") return "快照待更新，暂停预测";
  if (row.state === "insufficient") return "数据不足，暂无法预测";
  if (row.state !== "ready") return "预测未知";
  if (row.usedPercent === 100) return "快照时额度已用完";
  return row.runsOutAt === null
    ? `重置时预计已用 ${quotaPercent(row.projectedPercent)}`
    : "预计在重置前用完";
}
