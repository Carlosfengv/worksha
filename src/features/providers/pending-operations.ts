import {
  sameProviderRequest,
  type ProviderWriteRequest,
} from "./operation-receipt";
import {
  selectedProviderRoute,
  isExplicitProviderSource,
  sameProviderRoute,
  storedProviderSubmission,
  type ProviderWriteRoute,
} from "./catalog-operation";
const PREFIX = "local-agent.provider-operation.v1.";
export const PROVIDER_OPERATIONS_CHANGED = "provider-operations-changed";
const LIMIT = 4;
const MAX_CHARACTERS = 2 * 1024 * 1024;
// Retain only a digest for bytes outside the display budget. A prefix and length
// cannot prove an oversized record stayed unchanged while its dialog was open.
const fingerprints = new WeakMap<
  ProviderRecoveryRecord,
  Promise<ArrayBuffer | null>
>();
function fingerprint(raw: string) {
  try {
    return crypto.subtle
      .digest("SHA-256", new TextEncoder().encode(raw))
      .catch(() => null);
  } catch {
    return Promise.resolve(null);
  }
}
export interface ProviderRecoveryRecord {
  key: string;
  raw: string;
  rawLength: number;
  truncated: boolean;
  request: ProviderWriteRequest | null;
  catalog: ProviderWriteRoute | null;
}
export function providerRecoveryRecords() {
  const records: ProviderRecoveryRecord[] = [];
  let overflow = false;
  let characters = 0;
  for (const key of recoveryKeys(PREFIX)) {
    if (records.length >= LIMIT) {
      overflow = true;
      continue;
    }
    const raw = localStorage.getItem(key) ?? "";
    let request: ProviderWriteRequest | null = null;
    let catalog: ProviderWriteRoute | null = null;
    if (raw.length <= MAX_CHARACTERS) {
      try {
        const submission = storedProviderSubmission(JSON.parse(raw));
        request = submission?.request ?? null;
        catalog = submission?.catalog ?? null;
      } catch {
        /* Preserve corrupt records for explicit review. */
      }
    }
    if (request && key !== PREFIX + request.operationId) request = null;
    characters += raw.length;
    const retained = raw.slice(
      0,
      Math.max(0, MAX_CHARACTERS - (characters - raw.length)),
    );
    const record = {
      key,
      raw: retained,
      rawLength: raw.length,
      truncated: retained.length !== raw.length,
      request,
      catalog,
    };
    if (record.truncated) fingerprints.set(record, fingerprint(raw));
    records.push(record);
  }
  return { records, overflow, characters };
}
export function canStartProviderOperation(storeId: string, providerId: string) {
  const pending = providerRecoveryRecords();
  if (
    pending.overflow ||
    pending.records.length >= LIMIT ||
    pending.characters >= MAX_CHARACTERS
  )
    throw new Error("待核查服务商操作已达上限，请先处理恢复列表。");
  if (pending.records.some((row) => !row.request))
    throw new Error("存在不可读取的服务商恢复记录，请先核查，未自动丢弃。");
  if (
    pending.records.some(
      (row) =>
        row.request?.storeId === storeId &&
        row.request.provider.id === providerId,
    )
  )
    throw new Error("此服务商已有待核查操作，请先查询原结果。");
}
export function rememberProviderOperation(
  request: ProviderWriteRequest,
  catalog: ProviderWriteRoute | null = null,
) {
  canStartProviderOperation(request.storeId, request.provider.id);
  if (catalog && !selectedProviderRoute(catalog, request))
    throw Error("目录选择与原请求不一致，本次未提交。");
  // Keep the existing key namespace: older apps preserve the unfamiliar
  // envelope as unreadable and block new work, rather than replaying as native.
  const encoded = JSON.stringify(
    catalog
      ? isExplicitProviderSource(catalog)
        ? { version: 3, request, route: catalog }
        : { version: 2, request, catalog }
      : request,
  );
  if (
    !storedProviderSubmission(JSON.parse(encoded)) ||
    !fitsRecoveryBudget(
      encoded,
      MAX_CHARACTERS - providerRecoveryRecords().characters,
      "characters",
    )
  )
    throw new Error("恢复记录超过保存预算，本次未提交。");
  writeRecoveryRecord(
    PREFIX + request.operationId,
    encoded,
    MAX_CHARACTERS,
    "characters",
  );
  notifyRecoveryChanged(PROVIDER_OPERATIONS_CHANGED);
}
export function forgetProviderOperation(
  request: ProviderWriteRequest,
  catalog: ProviderWriteRoute | null = null,
) {
  const key = PREFIX + request.operationId;
  removeRecoveryRecord(key, (raw) => {
    let stored: ReturnType<typeof storedProviderSubmission> = null;
    try {
      stored = storedProviderSubmission(JSON.parse(raw));
    } catch {
      /* Keep the original bytes. */
    }
    return (
      !!stored &&
      sameProviderRequest(stored.request, request) &&
      sameProviderRoute(stored.catalog, catalog)
    );
  });
  notifyRecoveryChanged(PROVIDER_OPERATIONS_CHANGED);
}
// Recover transport before any query, retry or cancel. A damaged/replaced local
// envelope cannot silently become the legacy native submission.
export function providerOperationCatalog(
  request: ProviderWriteRequest,
  expected?: ProviderWriteRoute | null,
) {
  const raw = localStorage.getItem(PREFIX + request.operationId);
  if (raw === null) {
    if (expected && !selectedProviderRoute(expected, request))
      throw Error("原目录选择与请求不一致，未执行。");
    return expected ?? null;
  }
  let stored: ReturnType<typeof storedProviderSubmission> = null;
  if (raw.length <= MAX_CHARACTERS) {
    try {
      stored = storedProviderSubmission(JSON.parse(raw));
    } catch {
      /* Preserve unknown bytes. */
    }
  }
  if (
    !stored ||
    !sameProviderRequest(stored.request, request) ||
    (expected !== undefined && !sameProviderRoute(stored.catalog, expected))
  )
    throw Error(
      "原操作的目录选择或恢复记录已变化，请保留原记录核查；未改用内置请求。",
    );
  return stored.catalog;
}
// For the explicit manual-review dialog, including unreadable records. Never used after an automatic lookup failure.
export async function discardReviewedProviderRecord(
  record: ProviderRecoveryRecord,
) {
  const current = localStorage.getItem(record.key);
  if (
    !record.key.startsWith(PREFIX) ||
    current === null ||
    current.length !== record.rawLength
  )
    throw new Error("恢复记录已变化，请重新核查后处理。");
  if (record.truncated) {
    const [originalHash, currentHash] = await Promise.all([
      fingerprints.get(record),
      fingerprint(current),
    ]);
    if (
      !originalHash ||
      !currentHash ||
      !new Uint8Array(originalHash).every(
        (byte, index) => byte === new Uint8Array(currentHash)[index],
      )
    )
      throw new Error("恢复记录已变化或无法核查，请重新核查后处理。");
  } else if (current !== record.raw) {
    throw new Error("恢复记录已变化，请重新核查后处理。");
  }
  removeRecoveryRecord(
    record.key,
    // Re-read after hashing as another window may have replaced it meanwhile.
    (latest) => latest === current,
  );
  notifyRecoveryChanged(PROVIDER_OPERATIONS_CHANGED);
}
import {
  recoveryKeys,
  fitsRecoveryBudget,
  writeRecoveryRecord,
  removeRecoveryRecord,
  notifyRecoveryChanged,
} from "@/shared/lib/recovery-storage";
