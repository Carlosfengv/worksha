import { expect, test } from "@playwright/test";
import {
  freezeGatewayPending,
  freezeGatewayRequest,
  gatewayOutcome,
  gatewayOperationFileResult,
  gatewayRevocation,
} from "../../src/features/settings/gateway-operation";
import { gatewayStatus } from "../../src/features/settings/gateway-status";
import {
  gatewayFixtureOutcome,
  gatewayFixtureStatus,
  gatewayStoreA,
  gatewayClientId,
} from "./gateway-operation-fixture";
import type { PendingGatewayOperation } from "../../src/features/settings/types";
const pending: PendingGatewayOperation = {
  request: {
    storeId: gatewayStoreA,
    operationId: "33333333-3333-4333-8333-333333333333",
    action: {
      kind: "create",
      draft: {
        name: "fixture",
        agent: "claude",
        providerId: "fixture",
        models: ["fixture-model"],
        concurrency: 2,
      },
    },
  },
  compensationOperationId: "44444444-4444-4444-8444-444444444444",
};
const compensation = {
  storeId: gatewayStoreA,
  operationId: pending.compensationOperationId!,
  action: { kind: "revoke" as const, clientId: pending.request.operationId },
};
test("operation snapshots preserve exact request fields and reject extra private metadata", () => {
  const result = freezeGatewayPending(pending);
  expect(result).toEqual(pending);
  expect(result).not.toBe(pending);
  for (const value of [
    { ...pending, secret: "fixture" },
    { ...pending, compensationOperationId: pending.request.operationId },
    {
      ...pending,
      request: { ...pending.request, provider: { credentialRef: "fixture" } },
    },
  ])
    expect(() => freezeGatewayPending(value)).toThrow();
  expect(() =>
    freezeGatewayRequest({
      ...pending.request,
      action: {
        kind: "create",
        draft: {
          name: "bad\0name",
          agent: "claude",
          providerId: "fixture",
          models: ["fixture"],
          concurrency: 1,
        },
      },
    }),
  ).toThrow();
});
test("operation replies validate the original source, action, client and all receipt fields", () => {
  const outcome = gatewayFixtureOutcome(pending.request);
  expect(gatewayOutcome(outcome, pending.request)).toEqual(outcome);
  for (const value of [
    { ...outcome, key: "must-not-cross" },
    {
      ...outcome,
      receipt: { ...outcome.receipt, operationId: gatewayClientId },
    },
    { ...outcome, receipt: { ...outcome.receipt, clientId: gatewayClientId } },
    { ...outcome, receipt: { ...outcome.receipt, action: "revoke" } },
    { ...outcome, receipt: { ...outcome.receipt, completedAt: "not-a-date" } },
    {
      ...outcome,
      receipt: { ...outcome.receipt, listen: "other.invalid:1234" },
    },
    { ...outcome, request: { ...outcome.request, storeId: gatewayClientId } },
  ])
    expect(() => gatewayOutcome(value, pending.request)).toThrow();
  for (const listen of ["127.0.0.1:80", "[::1]:43210"])
    expect(
      gatewayOutcome(
        { ...outcome, receipt: { ...outcome.receipt, listen } },
        pending.request,
      ).receipt.listen,
    ).toBe(listen);
});
test("revocation current-state proof cannot impersonate an operation receipt or another target", () => {
  const committed = {
    status: "committed",
    outcome: gatewayFixtureOutcome(compensation),
  };
  expect(gatewayRevocation(committed, compensation)).toEqual(committed);
  const state = { status: "alreadyRevoked", request: compensation };
  expect(gatewayRevocation(state, compensation)).toEqual(state);
  for (const value of [
    { ...state, outcome: committed.outcome },
    { ...state, request: { ...compensation, operationId: gatewayClientId } },
    { status: "committed", outcome: gatewayFixtureOutcome(pending.request) },
  ])
    expect(() => gatewayRevocation(value, compensation)).toThrow();
});
test("file results keep issued, saved, not-issued, cancelled and compensated states distinct", () => {
  const metadata = {
    outcome: gatewayFixtureOutcome(pending.request),
    sourceInstanceId: gatewayClientId,
    sourceCurrent: false,
  };
  expect(
    gatewayOperationFileResult({ status: "issued", ...metadata }, pending)
      .status,
  ).toBe("issued");
  expect(
    gatewayOperationFileResult(
      { status: "saved", fileName: "client.json", ...metadata },
      pending,
    ).status,
  ).toBe("saved");
  expect(
    gatewayOperationFileResult(
      { status: "notIssued", reason: "fixture no send" },
      pending,
    ).status,
  ).toBe("notIssued");
  expect(
    gatewayOperationFileResult({ status: "cancelled" }, pending).status,
  ).toBe("cancelled");
  expect(
    gatewayOperationFileResult(
      {
        status: "revoked",
        reason: "fixture compensated",
        compensation: {
          status: "committed",
          outcome: gatewayFixtureOutcome(compensation),
        },
        ...metadata,
      },
      pending,
    ).status,
  ).toBe("revoked");
  for (const value of [
    { status: "saved", fileName: "../x", ...metadata },
    { status: "issued", fileName: "client.json", ...metadata },
    { status: "cancelled", outcome: metadata.outcome },
    { status: "notIssued", reason: "fixture", key: "private" },
    { status: "saved", id: gatewayClientId, fileName: "legacy.json" },
    { status: "issued", ...metadata, apiKey: "private" },
  ])
    expect(() => gatewayOperationFileResult(value, pending)).toThrow();
});
test("status projection preserves legacy reads, supported capabilities and true zero while rejecting false source data", () => {
  const status = gatewayFixtureStatus();
  expect(gatewayStatus(status)).toEqual(status);
  const legacy = { ...status };
  delete legacy.writeProtocol;
  expect(gatewayStatus(legacy).writeProtocol).toBeUndefined();
  for (const bad of [
    { ...status, writeProtocol: { ...status.writeProtocol, storeId: "other" } },
    { ...status, clients: [...status.clients, ...status.clients] },
    { ...status, active: "0" },
    { ...status, clients: [], key: "private" },
  ])
    expect(() => gatewayStatus(bad)).toThrow();
});
