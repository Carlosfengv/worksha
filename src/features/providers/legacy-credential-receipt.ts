import { DesktopError } from "@/shared/lib/desktop-command";
import type { CredentialIdentityV3 } from "./credential-receipt";
import { validCredentialLabel } from "./credential-recovery-records";
export interface LegacyCredentialPreview extends CredentialIdentityV3 {
  version: 2;
  sourceDigest: string;
  stage: "unclaimed" | "prepared" | "fenced" | "committed";
  phase: "pending" | "ready" | "cancelled_pending" | "cancelled";
  label: string | null;
}
export interface LegacyCredentialConfirmation extends CredentialIdentityV3 {
  sourceDigest: string;
  confirmed: true;
}
export const validLegacyDigest = (value: unknown): value is string =>
  typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
export function legacyCredentialPreview(
  value: unknown,
  identity: CredentialIdentityV3,
): LegacyCredentialPreview {
  const row =
    value && typeof value === "object"
      ? (value as Record<string, unknown>)
      : null;
  if (
    !row ||
    Object.keys(row).sort().join(",") !==
      "label,namespaceId,operationId,phase,sourceDigest,stage,storeId,version" ||
    row.version !== 2 ||
    row.storeId !== identity.storeId ||
    row.namespaceId !== identity.namespaceId ||
    row.operationId !== identity.operationId ||
    !validLegacyDigest(row.sourceDigest) ||
    !["unclaimed", "prepared", "fenced", "committed"].includes(
      String(row.stage),
    ) ||
    !["pending", "ready", "cancelled_pending", "cancelled"].includes(
      String(row.phase),
    ) ||
    (row.phase === "cancelled"
      ? row.label !== null
      : !validCredentialLabel(row.label))
  )
    throw new DesktopError({
      code: "transport_invalid_response",
      outcomeUnknown: true,
      message: "旧记录核查结果与原来源或材料不对应，未自动采用。",
    });
  return row as unknown as LegacyCredentialPreview;
}
