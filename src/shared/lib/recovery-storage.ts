type BudgetUnit = "utf8" | "characters";
const encoder = new TextEncoder();

// Domain modules own keys, identity checks, encodings and admission limits.
export function* recoveryKeys(prefix: string) {
  for (let index = 0; index < localStorage.length; index++) {
    const key = localStorage.key(index);
    if (key?.startsWith(prefix)) yield key;
  }
}
export function fitsRecoveryBudget(
  raw: string,
  limit: number,
  unit: BudgetUnit = "utf8",
) {
  return (
    Number.isSafeInteger(limit) &&
    limit >= 0 &&
    raw.length <= limit &&
    (unit === "characters" || encoder.encode(raw).length <= limit)
  );
}
export function writeRecoveryRecord(
  key: string,
  raw: string,
  limit: number,
  unit: BudgetUnit = "utf8",
) {
  if (!fitsRecoveryBudget(raw, limit, unit))
    throw Error("恢复记录超过保存预算，本次未提交。");
  if (localStorage.getItem(key) !== null)
    throw Error("该操作编号已有恢复记录，本次未覆盖或提交。");
  localStorage.setItem(key, raw);
  if (localStorage.getItem(key) !== raw)
    throw Error("原请求未能完整保存，本次未提交。");
}
export function removeRecoveryRecord(
  key: string,
  matches: (raw: string) => boolean,
) {
  const raw = localStorage.getItem(key);
  if (raw === null) return;
  if (!matches(raw)) throw Error("恢复记录已变化，未移除原记录。");
  localStorage.removeItem(key);
  if (localStorage.getItem(key) !== null)
    throw Error("恢复记录未能移除，请重新核查。");
}
export function notifyRecoveryChanged(event: string) {
  window.dispatchEvent(new Event(event));
}
