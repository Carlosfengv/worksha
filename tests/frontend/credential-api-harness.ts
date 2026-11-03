import { mockIPC } from "@tauri-apps/api/mocks";
import { providersApi } from "@/features/providers/api";
import { discoveryFixturePage } from "./credential-discovery-fixture";
declare global {
  interface Window {
    credentialClient: typeof providersApi;
    credentialCommands: { name: string; args: unknown }[];
  }
}
Object.defineProperty(globalThis, "isTauri", {
  value: true,
  configurable: true,
});
window.credentialCommands = [];
window.credentialClient = providersApi;
const mode = new URLSearchParams(location.search).get("mode");
mockIPC((name, args) => {
  window.credentialCommands.push({ name, args });
  const input = args as Record<string, unknown>;
  const journalNamespace = "7025064b-5ff9-4f40-8b55-946614da682b";
  const otherNamespace = "f776744f-9a29-4498-9588-fbf3bfb07444";
  if (name === "discover_provider_credentials") {
    const request = input.request as Record<string, unknown>;
    if (Object.keys(request).sort().join(",") !== "cursor,namespaceId,storeId")
      throw JSON.stringify({
        code: "invalid_credential_operation",
        outcomeUnknown: false,
        message: "发现请求字段不匹配",
      });
    return discoveryFixturePage(request, mode);
  }
  if (
    name === "preview_legacy_provider_credential" ||
    name === "confirm_legacy_provider_credential"
  ) {
    const request = input.request as Record<string, unknown>;
    const fields = Object.keys(request).sort().join(",");
    const expectedFields =
      name === "preview_legacy_provider_credential"
        ? "namespaceId,operationId,storeId"
        : "confirmed,namespaceId,operationId,sourceDigest,storeId";
    if (fields !== expectedFields)
      throw JSON.stringify({
        code: "invalid_credential_operation",
        outcomeUnknown: false,
        message: "旧记录请求字段不匹配",
      });
    const value = {
      version:
        mode === "legacy-api-future"
          ? 3
          : mode === "legacy-api-v1" ||
              (mode === "legacy-api-old-confirm" &&
                name === "confirm_legacy_provider_credential")
            ? 1
            : 2,
      storeId: request.storeId,
      namespaceId:
        mode === "legacy-api-foreign" ? otherNamespace : request.namespaceId,
      operationId: request.operationId,
      sourceDigest:
        mode === "legacy-api-bad-digest"
          ? "bad"
          : mode === "legacy-api-changed"
            ? "b".repeat(64)
            : "a".repeat(64),
      stage:
        name === "confirm_legacy_provider_credential"
          ? mode === "legacy-api-uncommitted"
            ? "fenced"
            : "committed"
          : "unclaimed",
      phase: "ready",
      label: "Legacy fixture",
    };
    return mode === "legacy-api-extra" ? { ...value, extra: true } : value;
  }
  if (name === "get_provider_credential_protocol_v3") {
    if (mode === "journal-unsupported")
      throw JSON.stringify({
        code: "credential_protocol_unavailable",
        outcomeUnknown: false,
        message: "旧后台",
      });
    return {
      version: mode === "journal-future-protocol" ? 4 : 3,
      storeId:
        mode === "journal-foreign-store"
          ? "6087597b-0625-42ea-8fd0-e51f6ad16d01"
          : input.storeId,
      namespaceId:
        mode === "journal-foreign-namespace"
          ? otherNamespace
          : journalNamespace,
      namespaceState: mode === "journal-unverified" ? "unverified" : "active",
      legacyState: "fresh",
      operationLimit: mode === "journal-bad-limit" ? 5000 : 4096,
    };
  }
  if (name === "initialize_provider_credential_namespace") {
    const request = input.request as Record<string, unknown>;
    return {
      version: 3,
      storeId: request.storeId,
      namespaceId: journalNamespace,
      namespaceState: "active",
      legacyState: request.choice === "restore" ? "imported" : "unverified",
      operationLimit: 4096,
    };
  }
  if (
    [
      "create_provider_credential_v3",
      "get_provider_credential_operation_v3",
      "cancel_provider_credential_operation_v3",
    ].includes(name)
  ) {
    const request = input.request as Record<string, unknown>;
    const value = {
      version: mode === "journal-bad-version" ? 4 : 3,
      storeId:
        mode === "journal-bad-store"
          ? "6087597b-0625-42ea-8fd0-e51f6ad16d01"
          : request.storeId,
      namespaceId:
        mode === "journal-bad-namespace" ? otherNamespace : request.namespaceId,
      operation: {
        operationId: mode === "journal-bad-id" ? "bad" : request.operationId,
        status: mode === "journal-bad-status" ? "cancelled" : "ready",
        receipt: {
          operationId: request.operationId,
          credentialRef:
            mode === "journal-bad-reference"
              ? "bad"
              : "54261267-b628-4be6-ab05-462c312a251c",
          credentialLabel:
            mode === "journal-bad-label" ? "Other label" : "Fixture",
        },
      },
    };
    return mode === "journal-extra-field" ? { ...value, extra: true } : value;
  }
  const receipt = {
    operationId: input.operationId,
    credentialRef: "e1b71318-84c0-4f5d-b399-aee182f69ade",
    credentialLabel: "Fixture",
  };
  if (name === "get_provider_credential_protocol") {
    if (mode === "scoped-unsupported")
      throw JSON.stringify({
        code: "credential_protocol_unavailable",
        outcomeUnknown: false,
        message: "旧后台",
      });
    return {
      version: 2,
      storeId:
        mode === "scoped-foreign"
          ? "6087597b-0625-42ea-8fd0-e51f6ad16d01"
          : input.storeId,
      operationLimit: 4096,
    };
  }
  if (
    name === "create_scoped_provider_credential" ||
    name === "get_scoped_provider_credential_operation" ||
    name === "cancel_scoped_provider_credential_operation"
  ) {
    const value = {
      version: mode === "scoped-bad-version" ? 3 : 2,
      storeId: input.storeId,
      operation: { operationId: input.operationId, status: "ready", receipt },
    };
    if (mode === "scoped-bad-id") value.operation.operationId = "wrong";
    if (mode === "scoped-bad-reference")
      value.operation.receipt.credentialRef = "bad";
    if (mode === "scoped-bad-status") value.operation.status = "cancelled";
    return value;
  }
  if (name === "get_provider_credential_operation") {
    if (mode === "unsupported")
      throw JSON.stringify({
        code: "credential_protocol_unavailable",
        message: "旧后台不支持核查",
      });
    if (mode === "conflict")
      return { operationId: input.operationId, status: "ready", receipt };
    throw JSON.stringify({ code: "operation_not_found", message: "尚未查到" });
  }
  if (mode === "conflict")
    throw JSON.stringify({
      code: "operation_conflict",
      message: "同编号内容不同",
    });
  return mode === "malformed" ? { ...receipt, operationId: "wrong" } : receipt;
});
