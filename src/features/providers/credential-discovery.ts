import { DesktopError } from "@/shared/lib/desktop-command";
import { isCredentialOperationId } from "./credential-receipt";
import { validCredentialLabel } from "./credential-recovery-records";
import { validLegacyDigest } from "./legacy-credential-receipt";

export interface CredentialDiscoveryRequest {
  storeId: string;
  namespaceId: string;
  cursor: string | null;
}
export interface DiscoveredCredential {
  kind: "legacy" | "scoped_v2" | "scoped_v3";
  operationId: string;
  digest: string | null;
  label: string | null;
  phase: "pending" | "ready" | "cancelled_pending" | "cancelled" | null;
  legacyStage: "unclaimed" | "prepared" | "fenced" | "committed" | null;
  issue: "unavailable" | "invalid" | "source_unverified" | "conflict" | null;
}
export interface CredentialDiscoveryPage {
  version: 1;
  storeId: string;
  namespaceId: string;
  snapshot: string;
  pageSize: 32;
  totalCandidates: number;
  scanned: number;
  merged: number;
  unrecognized: number;
  items: DiscoveredCredential[];
  nextCursor: string | null;
}
export const discoveredCredentialKey = (item: DiscoveredCredential) =>
  item.kind + ":" + item.operationId;
export const credentialDiscoveryCursor = (value: unknown): value is string =>
  typeof value === "string" &&
  /^[a-f0-9]{64}:[123]:[a-f0-9-]{36}$/.test(value) &&
  isCredentialOperationId(value.slice(67));
function object(value: unknown, fields: string): Record<string, unknown> {
  if (
    !value ||
    typeof value !== "object" ||
    Array.isArray(value) ||
    Object.keys(value).sort().join(",") !== fields
  )
    throw Error();
  return value as Record<string, unknown>;
}
export function credentialDiscoveryPage(
  value: unknown,
  request: CredentialDiscoveryRequest,
): CredentialDiscoveryPage {
  try {
    const row = object(
      value,
      "items,merged,namespaceId,nextCursor,pageSize,scanned,snapshot,storeId,totalCandidates,unrecognized,version",
    );
    if (
      row.version !== 1 ||
      row.storeId !== request.storeId ||
      row.namespaceId !== request.namespaceId ||
      !validLegacyDigest(row.snapshot) ||
      row.pageSize !== 32 ||
      !Array.isArray(row.items) ||
      row.items.length > 32
    )
      throw Error();
    for (const key of ["totalCandidates", "scanned", "merged", "unrecognized"])
      if (
        !Number.isSafeInteger(row[key]) ||
        Number(row[key]) < 0 ||
        Number(row[key]) > 12290
      )
        throw Error();
    if (
      Number(row.totalCandidates) > 12288 ||
      Number(row.scanned) > 32 ||
      Number(row.merged) > Number(row.scanned) ||
      Number(row.scanned) > Number(row.totalCandidates) ||
      (!request.cursor &&
        row.scanned !== Math.min(32, Number(row.totalCandidates))) ||
      row.items.length + Number(row.merged) !== row.scanned ||
      (row.nextCursor !== null &&
        (!credentialDiscoveryCursor(row.nextCursor) ||
          !row.nextCursor.startsWith(row.snapshot + ":") ||
          row.scanned !== 32))
    )
      throw Error();
    if (request.cursor && !request.cursor.startsWith(row.snapshot + ":"))
      throw Error();
    const keys = new Set<string>();
    let last = "";
    for (const value of row.items) {
      const item = object(
        value,
        "digest,issue,kind,label,legacyStage,operationId,phase",
      );
      if (
        !["legacy", "scoped_v2", "scoped_v3"].includes(String(item.kind)) ||
        !isCredentialOperationId(item.operationId)
      )
        throw Error();
      const key =
        String(
          ["legacy", "scoped_v2", "scoped_v3"].indexOf(String(item.kind)) + 1,
        ) +
        ":" +
        item.operationId;
      if (
        keys.has(key) ||
        key <= last ||
        (request.cursor && key <= request.cursor.slice(65))
      )
        throw Error();
      keys.add(key);
      last = key;
      if (item.issue !== null) {
        if (
          !["unavailable", "invalid", "source_unverified", "conflict"].includes(
            String(item.issue),
          ) ||
          [item.digest, item.label, item.phase, item.legacyStage].some(
            (v) => v !== null,
          )
        )
          throw Error();
      } else {
        if (
          !validLegacyDigest(item.digest) ||
          !["pending", "ready", "cancelled_pending", "cancelled"].includes(
            String(item.phase),
          ) ||
          (item.phase === "cancelled"
            ? item.label !== null
            : !validCredentialLabel(item.label)) ||
          (typeof item.label === "string" && item.label.trim() !== item.label)
        )
          throw Error();
        if (
          item.kind === "legacy"
            ? item.legacyStage !== "unclaimed"
            : item.kind === "scoped_v2"
              ? item.legacyStage !== null
              : item.legacyStage !== null &&
                !["prepared", "fenced", "committed"].includes(
                  String(item.legacyStage),
                )
        )
          throw Error();
      }
    }
    if (
      typeof row.nextCursor === "string" &&
      (row.nextCursor.slice(65) < last ||
        (request.cursor && row.nextCursor <= request.cursor))
    )
      throw Error();
    return row as unknown as CredentialDiscoveryPage;
  } catch {
    throw new DesktopError({
      code: "transport_invalid_response",
      outcomeUnknown: false,
      message: "后台记录列表的来源或格式无法核查，未重建提示。",
    });
  }
}
