import { DesktopError } from "@/shared/lib/desktop-command";
import { isUuidV4 } from "@/shared/lib/uuid";
import type {
  GatewayOperationRequest,
  GatewayOperationOutcome,
  GatewayOperationFileResult,
  PendingGatewayOperation,
  GatewayRevocation,
} from "./types";
export const gatewayUuid = (value: unknown): value is string =>
  typeof value === "string" &&
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);
export const gatewayUuid4 = isUuidV4;
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw Error("客户端数据不可读取");
  return value as Record<string, unknown>;
}
function exact(row: Record<string, unknown>, keys: string[]) {
  if (Object.keys(row).some((key) => !keys.includes(key)))
    throw Error("客户端数据包含未知字段");
}
const text = (value: unknown, max: number): value is string =>
  typeof value === "string" &&
  value.length > 0 &&
  new TextEncoder().encode(value).length <= max;
const invalid = () =>
  new DesktopError({
    code: "transport_invalid_response",
    outcomeUnknown: true,
    message: "客户端原结果不可核对，请保留原编号核查；未重新签发密钥",
  });
export function freezeGatewayRequest(value: unknown): GatewayOperationRequest {
  const row = object(value);
  exact(row, ["storeId", "operationId", "action"]);
  if (!gatewayUuid4(row.storeId) || !gatewayUuid4(row.operationId))
    throw Error("客户端原编号或来源无效");
  const action = object(row.action);
  let result: GatewayOperationRequest;
  if (action.kind === "create") {
    exact(action, ["kind", "draft"]);
    const d = object(action.draft);
    exact(d, ["name", "agent", "providerId", "models", "concurrency"]);
    if (
      !text(d.name, 320) ||
      !d.name.trim() ||
      [...d.name].length > 80 ||
      /\p{Cc}/u.test(d.name) ||
      !["claude", "codex", "opencode"].includes(String(d.agent)) ||
      !text(d.providerId, 65536) ||
      !Array.isArray(d.models) ||
      !d.models.length ||
      d.models.length > 200 ||
      !d.models.every((m) => text(m, 256)) ||
      !Number.isInteger(d.concurrency) ||
      (d.concurrency as number) < 1 ||
      (d.concurrency as number) > 16
    )
      throw Error("客户端原草稿无效");
    result = {
      storeId: row.storeId,
      operationId: row.operationId,
      action: {
        kind: "create",
        draft: {
          name: d.name,
          agent: d.agent as "claude" | "codex" | "opencode",
          providerId: d.providerId,
          models: [...d.models] as string[],
          concurrency: d.concurrency as number,
        },
      },
    };
  } else {
    exact(action, ["kind", "clientId"]);
    if (action.kind !== "revoke" || !gatewayUuid(action.clientId))
      throw Error("客户端撤销身份无效");
    result = {
      storeId: row.storeId,
      operationId: row.operationId,
      action: { kind: "revoke", clientId: action.clientId },
    };
  }
  if (new TextEncoder().encode(JSON.stringify(result)).length > 65536)
    throw Error("客户端原请求超过容量");
  return result;
}
export function freezeGatewayPending(value: unknown): PendingGatewayOperation {
  const row = object(value);
  exact(row, ["request", "compensationOperationId"]);
  const request = freezeGatewayRequest(row.request);
  if (request.action.kind === "create") {
    if (
      !gatewayUuid4(row.compensationOperationId) ||
      row.compensationOperationId === request.operationId
    )
      throw Error("客户端原补偿编号无效");
    return { request, compensationOperationId: row.compensationOperationId };
  }
  if (row.compensationOperationId !== undefined)
    throw Error("撤销请求包含多余身份");
  return { request };
}
export const sameGatewayRequest = (
  a: GatewayOperationRequest,
  b: GatewayOperationRequest,
) =>
  JSON.stringify(freezeGatewayRequest(a)) ===
  JSON.stringify(freezeGatewayRequest(b));
export function gatewayCompensation(
  pending: PendingGatewayOperation,
): GatewayOperationRequest {
  const row = freezeGatewayPending(pending);
  if (row.request.action.kind !== "create" || !row.compensationOperationId)
    throw Error("没有保留原签发补偿编号");
  return {
    storeId: row.request.storeId,
    operationId: row.compensationOperationId,
    action: { kind: "revoke", clientId: row.request.operationId },
  };
}
export function gatewayOutcome(
  value: unknown,
  expected?: GatewayOperationRequest,
): GatewayOperationOutcome {
  try {
    const row = object(value);
    exact(row, ["request", "receipt"]);
    const request = freezeGatewayRequest(row.request);
    const r = object(row.receipt);
    exact(r, [
      "apiVersion",
      "storeId",
      "operationId",
      "action",
      "clientId",
      "listen",
      "completedAt",
    ]);
    if (
      r.apiVersion !== 1 ||
      r.storeId !== request.storeId ||
      r.operationId !== request.operationId ||
      r.action !== request.action.kind ||
      r.clientId !==
        (request.action.kind === "create"
          ? request.operationId
          : request.action.clientId) ||
      !text(r.listen, 128) ||
      !text(r.completedAt, 64) ||
      !Number.isFinite(Date.parse(r.completedAt)) ||
      (expected && !sameGatewayRequest(request, expected))
    )
      throw invalid();
    const url = new URL("http://" + r.listen),
      colon = r.listen.lastIndexOf(":"),
      host = r.listen.slice(0, colon),
      port = Number(r.listen.slice(colon + 1));
    if (
      !Number.isInteger(port) ||
      port < 1 ||
      port > 65535 ||
      !(/^[0-9.]+$/.test(host) || host.startsWith("[")) ||
      url.hostname !== host ||
      url.pathname !== "/" ||
      url.username ||
      url.password ||
      url.search ||
      url.hash
    )
      throw invalid();
    return {
      request,
      receipt: {
        apiVersion: 1,
        storeId: request.storeId,
        operationId: request.operationId,
        action: request.action.kind,
        clientId: r.clientId as string,
        listen: r.listen,
        completedAt: r.completedAt,
      },
    };
  } catch {
    throw invalid();
  }
}
export function gatewayOperationFileResult(
  value: unknown,
  pending: PendingGatewayOperation,
): GatewayOperationFileResult {
  try {
    const row = object(value);
    if (row.status === "cancelled") {
      exact(row, ["status"]);
      return { status: "cancelled" };
    }
    if (row.status === "notIssued") {
      exact(row, ["status", "reason"]);
      if (!text(row.reason, 4096)) throw invalid();
      return { status: "notIssued", reason: row.reason };
    }
    exact(row, [
      "status",
      "outcome",
      "sourceInstanceId",
      "sourceCurrent",
      "fileName",
      "compensation",
      "reason",
    ]);
    if (
      !gatewayUuid4(row.sourceInstanceId) ||
      typeof row.sourceCurrent !== "boolean"
    )
      throw invalid();
    const outcome = gatewayOutcome(row.outcome, pending.request);
    const source = {
      sourceInstanceId: row.sourceInstanceId,
      sourceCurrent: row.sourceCurrent,
      outcome,
    };
    if (
      row.status === "saved" &&
      text(row.fileName, 1024) &&
      !/[\0/]/.test(row.fileName) &&
      row.compensation === undefined &&
      row.reason === undefined
    )
      return { ...source, status: "saved", fileName: row.fileName };
    if (
      row.status === "issued" &&
      row.fileName === undefined &&
      row.compensation === undefined &&
      row.reason === undefined
    )
      return { ...source, status: "issued" };
    if (
      row.status === "revoked" &&
      row.fileName === undefined &&
      text(row.reason, 4096)
    )
      return {
        ...source,
        status: "revoked",
        compensation: gatewayRevocation(
          row.compensation,
          gatewayCompensation(pending),
        ),
        reason: row.reason,
      };
    throw invalid();
  } catch {
    throw invalid();
  }
}
export function gatewayRevocation(
  value: unknown,
  expected: GatewayOperationRequest,
): GatewayRevocation {
  try {
    const row = object(value);
    if (expected.action.kind !== "revoke") throw invalid();
    if (row.status === "committed") {
      exact(row, ["status", "outcome"]);
      return {
        status: "committed",
        outcome: gatewayOutcome(row.outcome, expected),
      };
    }
    exact(row, ["status", "request"]);
    const request = freezeGatewayRequest(row.request);
    if (
      row.status !== "alreadyRevoked" ||
      !sameGatewayRequest(request, expected)
    )
      throw invalid();
    return { status: "alreadyRevoked", request };
  } catch {
    throw invalid();
  }
}
