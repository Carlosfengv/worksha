import { DesktopError } from "@/shared/lib/desktop-command";
import {
  retentionInventory,
  retentionSourceReport,
  type RetentionInventory,
  type SourceReport,
} from "./retention-inventory";
import type { RetentionRequest } from "./retention-result";

export type ActivityImpact = {
  records: number;
  olderThanCutoff: number;
  withinWindow: number;
  unknownTime: number;
  fileBytes: number;
};
export type RetentionContentInventory = Omit<
  RetentionInventory,
  "version" | "scope"
> & {
  version: 2;
  scope: "product_owned_saved_sources";
  activityCaches: {
    gateway: SourceReport<ActivityImpact>;
    native: SourceReport<ActivityImpact>;
  };
};
function invalid(): never {
  throw new DesktopError({
    code: "retention_invalid_response",
    message: "活动摘要留存预览尚未通过核对，请重新预览。",
    outcomeUnknown: false,
  });
}
function object(v: unknown): Record<string, unknown> {
  if (!v || typeof v !== "object" || Array.isArray(v)) invalid();
  return v as Record<string, unknown>;
}
function activityImpact(value: unknown): ActivityImpact {
  const v = object(value);
  const names = [
    "records",
    "olderThanCutoff",
    "withinWindow",
    "unknownTime",
    "fileBytes",
  ];
  if (
    Object.keys(v).length !== names.length ||
    !names.every(
      (k) =>
        typeof v[k] === "number" && Number.isSafeInteger(v[k]) && v[k] >= 0,
    )
  )
    invalid();
  const a = v as ActivityImpact;
  if (
    a.records > 512 ||
    a.fileBytes > 2 * 1024 * 1024 ||
    a.fileBytes === 0 ||
    a.olderThanCutoff + a.withinWindow + a.unknownTime !== a.records
  )
    invalid();
  return a;
}
export function retentionContentInventory(
  value: unknown,
  request: RetentionRequest,
): RetentionContentInventory {
  const v = object(value);
  if (v.version !== 2 || v.scope !== "product_owned_saved_sources") invalid();
  const { activityCaches, ...common } = v;
  const caches = object(activityCaches);
  if (
    Object.keys(caches).length !== 2 ||
    !Object.hasOwn(caches, "gateway") ||
    !Object.hasOwn(caches, "native")
  )
    invalid();
  // Keep all legacy metadata/count checks; only the negotiated scope changes.
  const metadata = retentionInventory(
    { ...common, version: 1, scope: "product_owned_saved_metadata" },
    request,
  );
  return {
    ...metadata,
    version: 2,
    scope: "product_owned_saved_sources",
    activityCaches: {
      gateway: retentionSourceReport(caches.gateway, activityImpact, metadata),
      native: retentionSourceReport(caches.native, activityImpact, metadata),
    },
  };
}
