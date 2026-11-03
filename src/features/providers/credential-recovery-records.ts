import {
  isCredentialOperationId,
  type RecoverableCredentialReceipt,
} from "./credential-receipt";

export const CREDENTIAL_RECORD_PREFIXES = [
  "local-agent.credential-operation.v1.",
  "local-agent.credential-operation.v2.",
  "local-agent.credential-operation.v3.",
] as const;
export const CREDENTIAL_RECORD_LIMIT = 16;
export interface CredentialRecoveryRecord {
  version: 1 | 2 | 3;
  operationId: string;
  storeId?: string;
  namespaceId?: string;
  legacySourceDigest?: string;
  label: string;
  storageKey: string;
  raw: string;
  rawLength: number;
  issue?: string;
  receipt?: RecoverableCredentialReceipt;
  cancelledPending?: boolean;
}
export const validCredentialLabel = (label: unknown): label is string =>
  typeof label === "string" &&
  !!label.trim() &&
  new TextEncoder().encode(label).length <= 200 &&
  !/\p{Cc}/u.test(label);
export function readCredentialRecords(): CredentialRecoveryRecord[] {
  const result: CredentialRecoveryRecord[] = [];
  for (let n = 0; n < localStorage.length; n++) {
    const key = localStorage.key(n);
    if (!key) continue;
    const index = CREDENTIAL_RECORD_PREFIXES.findIndex((prefix) =>
      key.startsWith(prefix),
    );
    if (index < 0) continue;
    const version = (index + 1) as 1 | 2 | 3,
      parts = key.slice(CREDENTIAL_RECORD_PREFIXES[index].length).split("."),
      raw = localStorage.getItem(key) ?? "",
      operationId = parts[version - 1] ?? "";
    const base = {
      version,
      operationId,
      label: "待人工核查的凭据记录",
      storageKey: key,
      raw: raw.slice(0, 4096),
      rawLength: raw.length,
    };
    try {
      if (
        raw.length > 4096 ||
        parts.length !== version ||
        !parts.every(isCredentialOperationId)
      )
        throw Error();
      const row: unknown = JSON.parse(raw);
      if (
        !row ||
        typeof row !== "object" ||
        !("version" in row) ||
        row.version !== version ||
        !("label" in row) ||
        !validCredentialLabel(row.label)
      )
        throw Error();
      if (version > 1) {
        const names =
          version === 3
            ? "cancelledPending,label,namespaceId,operationId,storeId,version"
            : "cancelledPending,label,operationId,storeId,version";
        const legacyDigest =
          "legacySourceDigest" in row ? row.legacySourceDigest : undefined;
        const expectedNames =
          legacyDigest === undefined
            ? names
            : "cancelledPending,label,legacySourceDigest,namespaceId,operationId,storeId,version";
        if (
          Object.keys(row).sort().join(",") !== expectedNames ||
          (legacyDigest !== undefined &&
            (version !== 3 ||
              typeof legacyDigest !== "string" ||
              !/^[a-f0-9]{64}$/.test(legacyDigest))) ||
          !("storeId" in row) ||
          row.storeId !== parts[0] ||
          !("operationId" in row) ||
          row.operationId !== operationId ||
          !("cancelledPending" in row) ||
          typeof row.cancelledPending !== "boolean" ||
          row.label !== row.label.trim() ||
          (version === 3 &&
            (!("namespaceId" in row) || row.namespaceId !== parts[1]))
        )
          throw Error();
      }
      result.push({
        ...base,
        label: row.label,
        storeId: version > 1 ? parts[0] : undefined,
        namespaceId: version === 3 ? parts[1] : undefined,
        legacySourceDigest:
          "legacySourceDigest" in row
            ? (row.legacySourceDigest as string)
            : undefined,
        cancelledPending:
          "cancelledPending" in row && row.cancelledPending === true,
      });
    } catch {
      result.push({
        ...base,
        issue: "记录格式或原恢复空间无法核查，原字节已保留。",
      });
    }
    if (result.length >= CREDENTIAL_RECORD_LIMIT) break;
  }
  return result;
}
export function rememberCredentialRecord(record: {
  version: 2 | 3;
  storeId: string;
  namespaceId?: string;
  legacySourceDigest?: string;
  operationId: string;
  label: string;
  cancelledPending?: boolean;
}): CredentialRecoveryRecord {
  if (
    !isCredentialOperationId(record.storeId) ||
    !isCredentialOperationId(record.operationId) ||
    (record.version === 3 && !isCredentialOperationId(record.namespaceId)) ||
    (record.legacySourceDigest !== undefined &&
      (record.version !== 3 ||
        !/^[a-f0-9]{64}$/.test(record.legacySourceDigest))) ||
    !validCredentialLabel(record.label)
  )
    throw Error("凭据恢复信息无效，未覆盖原记录。");
  const key =
    CREDENTIAL_RECORD_PREFIXES[record.version - 1] +
    record.storeId +
    "." +
    (record.version === 3 ? record.namespaceId + "." : "") +
    record.operationId;
  const row = {
    version: record.version,
    storeId: record.storeId,
    operationId: record.operationId,
    label: record.label,
    cancelledPending: record.cancelledPending === true,
    ...(record.version === 3 ? { namespaceId: record.namespaceId } : {}),
    ...(record.legacySourceDigest
      ? { legacySourceDigest: record.legacySourceDigest }
      : {}),
  };
  const old = readCredentialRecords().find((value) => value.storageKey === key);
  if (old && (old.issue || old.label !== record.label))
    throw Error("原凭据恢复信息不同，未覆盖。");
  const raw = JSON.stringify(row);
  localStorage.setItem(key, raw);
  if (localStorage.getItem(key) !== raw)
    throw Error("凭据恢复信息未能保存，未发送密钥。");
  return { ...record, storageKey: key, raw, rawLength: raw.length };
}
export function forgetCredentialRecord(record: CredentialRecoveryRecord) {
  const raw = localStorage.getItem(record.storageKey);
  if (
    raw !== null &&
    (raw.length !== record.rawLength || raw.slice(0, 4096) !== record.raw)
  )
    throw Error("本机恢复记录已变化，请重新核查。");
  localStorage.removeItem(record.storageKey);
}
