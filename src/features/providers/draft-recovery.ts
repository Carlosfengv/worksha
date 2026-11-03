import {
  recoveryKeys,
  removeRecoveryRecord,
} from "@/shared/lib/recovery-storage";
import type { DraftGrantOwner } from "../../../contracts/domain/provider-draft-grant/types";
export type {
  DraftRecoveryEntry,
  DraftRecoveryState,
  DraftRecoveryOwnership,
} from "../../../contracts/domain/provider-draft-recovery/types";

const PREFIX = "local-agent.provider-draft-owner.v1.";
const id = (value: unknown): value is string =>
  typeof value === "string" &&
  /^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(value);
export interface DraftHint {
  key: string;
  raw: string;
  ownerId: string | null;
  storeId: string | null;
  oversized: boolean;
}
function hint(key: string, raw: string): DraftHint {
  const ownerId = key.slice(PREFIX.length);
  let storeId: string | null = null;
  try {
    const value = JSON.parse(raw) as { owner?: DraftGrantOwner } | null;
    if (value?.owner?.ownerId === ownerId && id(value.owner.storeId))
      storeId = value.owner.storeId;
  } catch {
    /* Display an unknown hint, never infer or delete its authority. */
  }
  return {
    key,
    raw,
    ownerId: id(ownerId) ? ownerId : null,
    storeId,
    oversized: new TextEncoder().encode(raw).length > 16384,
  };
}
export function draftGrantHints() {
  const rows: DraftHint[] = [];
  let bytes = 0,
    overflow = false;
  for (const key of recoveryKeys(PREFIX)) {
    if (rows.length >= 128) {
      overflow = true;
      break;
    }
    const raw = localStorage.getItem(key);
    if (raw === null) continue;
    bytes += new TextEncoder().encode(raw).length;
    rows.push(hint(key, raw));
    if (bytes > 512 * 1024) {
      overflow = true;
      break;
    }
  }
  return { rows, overflow };
}
export function draftGrantHint(owner: DraftGrantOwner) {
  const key = PREFIX + owner.ownerId,
    raw = localStorage.getItem(key);
  return raw === null ? null : hint(key, raw);
}
// Only a user-confirmed exact hint snapshot is removed. Host permission and
// the original Provider recovery envelope have independent lifecycles.
export function removeDraftGrantHint(snapshot: DraftHint) {
  removeRecoveryRecord(snapshot.key, (raw) => raw === snapshot.raw);
}
