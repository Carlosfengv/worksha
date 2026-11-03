import type {
  GatewayOperationRequest,
  GatewayOperationOutcome,
  GatewayStatus,
} from "@/features/settings/types";
export const gatewayStoreA = "00000000-0000-4000-8000-000000000001";
export const gatewayStoreB = "00000000-0000-4000-8000-000000000002";
export const gatewayClientId = "11111111-1111-4111-8111-111111111111";
export function gatewayFixtureOutcome(
  request: GatewayOperationRequest,
): GatewayOperationOutcome {
  return {
    request: structuredClone(request),
    receipt: {
      apiVersion: 1,
      storeId: request.storeId,
      operationId: request.operationId,
      action: request.action.kind,
      clientId:
        request.action.kind === "create"
          ? request.operationId
          : request.action.clientId,
      listen: "127.0.0.1:43210",
      completedAt: "2026-10-27T00:00:00Z",
    },
  };
}
export function gatewayFixtureStatus(storeId = gatewayStoreA): GatewayStatus {
  return {
    listen: "127.0.0.1:43210",
    persistent: true,
    concurrency: 16,
    active: 0,
    clients: [
      {
        id: gatewayClientId,
        name: "Laptop A",
        agent: "claude",
        providerId: "fixture",
        models: ["fixture-model"],
        concurrency: 2,
        active: 0,
        enabled: true,
        createdAt: "2026-10-27T00:00:00Z",
      },
    ],
    writeProtocol: {
      version: 1,
      storeId,
      maxClients: 128,
      secretReplay: false,
    },
  };
}
