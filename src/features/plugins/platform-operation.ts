import type { PlatformActionRequest } from "../../../contracts/plugins/platform/types";
import { DesktopError } from "../../shared/lib/desktop-command.ts";
import {
  removeRecoveryRecord,
  writeRecoveryRecord,
} from "../../shared/lib/recovery-storage.ts";

function matchesReceipt(result: unknown, request: PlatformActionRequest) {
  if (
    !result ||
    typeof result !== "object" ||
    "pending" in result ||
    !("grantRevision" in result) ||
    result.grantRevision !== request.expectedGrantRevision ||
    !("recordId" in result)
  )
    return false;
  if (request.actionId === "records.create")
    return typeof result.recordId === "string" && result.recordId.length > 0;
  if (request.actionId === "records.delete") return result.recordId === null;
  return result.recordId === (request.recordId ?? null);
}

export async function submitPlatformAction(
  request: PlatformActionRequest,
  storeId: string | null,
  send: (body: PlatformActionRequest) => Promise<unknown>,
) {
  const { digest, expectedGrantRevision, expectedEpoch, ...intent } = request;
  const canonical = JSON.stringify(intent, (_, value: unknown) =>
    value && typeof value === "object" && !Array.isArray(value)
      ? Object.fromEntries(
          Object.entries(value).sort(([a], [b]) =>
            a < b ? -1 : a > b ? 1 : 0,
          ),
        )
      : value,
  );
  const fingerprint = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(canonical),
  );
  const key =
    "local-agent.plugin-action.v1." +
    Array.from(new Uint8Array(fingerprint), (n) =>
      n.toString(16).padStart(2, "0"),
    ).join("");
  const scope = JSON.stringify([
    storeId,
    digest,
    expectedGrantRevision,
    expectedEpoch,
  ]);
  const stored = localStorage.getItem(key);
  let operationId: string;
  if (stored !== null) {
    const saved: unknown = JSON.parse(stored);
    if (
      !Array.isArray(saved) ||
      saved[0] !== scope ||
      typeof saved[1] !== "string" ||
      !storeId
    )
      throw new Error("此操作仍有待核查的提交，请在原连接和授权下核对记录。");
    operationId = saved[1];
  } else {
    operationId = crypto.randomUUID();
    writeRecoveryRecord(key, JSON.stringify([scope, operationId]), 2048);
  }
  const forget = () =>
    removeRecoveryRecord(
      key,
      (raw) => raw === JSON.stringify([scope, operationId]),
    );
  const body = storeId ? { ...request, storeId, operationId } : request;
  try {
    if (stored !== null) {
      const receipt = await send({ ...body, receiptOnly: true });
      if (matchesReceipt(receipt, request)) {
        forget();
        return receipt;
      }
      if (
        !receipt ||
        typeof receipt !== "object" ||
        "recordId" in receipt ||
        !("pending" in receipt) ||
        receipt.pending !== true
      )
        throw new Error("插件操作回执无法确认，请核查原记录。");
    }
    const result = await send(body);
    if (storeId && !matchesReceipt(result, request))
      throw new Error("插件操作回执无法确认，请核查原记录。");
    forget();
    return result;
  } catch (error) {
    if (
      !stored &&
      error instanceof DesktopError &&
      !error.outcomeUnknown &&
      [
        "invalid_input",
        "permission_denied",
        "grant_revoked",
        "resource_limit",
        "not_found",
        "record_conflict",
      ].includes(error.code)
    )
      forget();
    throw error;
  }
}
