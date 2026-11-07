import { DesktopError } from "@/shared/lib/desktop-command";
import { backendReadEpoch } from "@/shared/lib/backend-recovery";
import {
  providerOperationId,
  validatedProviderMetadata,
} from "./operation-receipt";
import {
  credentialOperation,
  type CredentialOperation,
  type CredentialIdentityV3,
} from "./credential-receipt";
import type { NativeConfig, Provider, ProviderTransaction } from "./types";

export type LocalTokenIdentity = CredentialIdentityV3;
export class LocalTokenPreparationError extends DesktopError {
  readonly identity: LocalTokenIdentity;
  constructor(reason: unknown, identity: LocalTokenIdentity) {
    super(reason);
    this.identity = { ...identity };
  }
}
export interface LocalTokenSource {
  identity: LocalTokenIdentity;
  epoch: symbol;
  active: boolean;
}
export type LocalTokenPurpose =
  "fixed_route" | "dynamic_binding" | "model_service";
export interface LocalTokenOperation {
  version: 1;
  status: "pending" | "committed" | "cancelled";
  request: {
    identity: LocalTokenIdentity;
    purpose: LocalTokenPurpose;
    agentId: string;
    configurationPath: string;
    configurationPathHash: string;
    expectedRevision: string;
    provider: Provider;
    service: { id: string; versionId: string; revision: number } | null;
  };
  ownerId: string;
  credentialRef: string;
  ownerSaved: boolean;
  previewId: string | null;
}
export interface LocalTokenProtocol {
  version: 2;
  schemaVersion: number;
  storeId: string;
  namespaceId: string;
  namespaceState: "active" | "unverified";
  operationLimit: 4096;
  pendingLimit: 4;
  cancellationLimit: 4096;
  pending: LocalTokenOperation[];
}
export interface LocalTokenResult {
  version: 2;
  identity: LocalTokenIdentity;
  operation: LocalTokenOperation | null;
  credential: CredentialOperation | null;
  preview: ProviderTransaction | null;
  cancellation: { version: 1; identity: LocalTokenIdentity } | null;
  canContinue: boolean;
  sourceMessage: string;
}
export interface LocalTokenHint {
  version: 1;
  identity: LocalTokenIdentity;
  purpose: LocalTokenPurpose;
  agentId: string;
  expectedRevision: string;
  providerId: string;
  providerRevision?: number;
  model?: string;
  serviceId?: string;
  serviceRevision?: number;
}
const object = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === "object" && !Array.isArray(value);
const keys = (value: Record<string, unknown>, allowed: string[]) =>
  Object.keys(value).every((key) => allowed.includes(key));
const encoder = new TextEncoder();
const text = (value: unknown, max: number): value is string =>
  typeof value === "string" &&
  encoder.encode(value).length <= max &&
  !/[\u0000-\u001f\u007f]/u.test(value);
const integer = (value: unknown): value is number =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const agent = (value: unknown): value is string =>
  typeof value === "string" &&
  ["claude", "codex", "opencode", "claude-desktop"].includes(value);
const purpose = (value: unknown): value is LocalTokenPurpose =>
  typeof value === "string" &&
  ["fixed_route", "dynamic_binding", "model_service"].includes(value);
const revision = (value: unknown): value is string =>
  value === "absent" ||
  (typeof value === "string" && /^[a-f0-9]{64}$/u.test(value));
const failure = (mutation = true) =>
  new DesktopError({
    code: "local_token_protocol_unavailable",
    outcomeUnknown: mutation,
    message: "原本机入口回复或来源不可核查，请保留原操作编号；未重新创建入口。",
  });
export function localTokenIdentity(value: unknown): LocalTokenIdentity {
  if (
    !object(value) ||
    !keys(value, ["storeId", "namespaceId", "operationId"]) ||
    !providerOperationId(value.storeId) ||
    !providerOperationId(value.namespaceId) ||
    !providerOperationId(value.operationId)
  )
    throw failure(false);
  return {
    storeId: value.storeId,
    namespaceId: value.namespaceId,
    operationId: value.operationId,
  };
}
export const sameLocalTokenIdentity = (
  left: LocalTokenIdentity,
  right: LocalTokenIdentity,
) =>
  left.storeId === right.storeId &&
  left.namespaceId === right.namespaceId &&
  left.operationId === right.operationId;
export function localTokenOperation(value: unknown): LocalTokenOperation {
  if (
    !object(value) ||
    !keys(value, [
      "version",
      "status",
      "request",
      "ownerId",
      "credentialRef",
      "ownerSaved",
      "previewId",
    ]) ||
    value.version !== 1 ||
    !["pending", "committed", "cancelled"].includes(String(value.status)) ||
    !object(value.request)
  )
    throw failure();
  const request = value.request;
  if (
    !keys(request, [
      "identity",
      "purpose",
      "agentId",
      "configurationPath",
      "configurationPathHash",
      "expectedRevision",
      "provider",
      "service",
    ]) ||
    !purpose(request.purpose) ||
    !agent(request.agentId) ||
    (request.purpose !== "fixed_route" &&
      !["claude", "opencode"].includes(request.agentId)) ||
    !text(request.configurationPath, 16384) ||
    !request.configurationPath ||
    !revision(request.configurationPathHash) ||
    request.configurationPathHash === "absent" ||
    !revision(request.expectedRevision)
  )
    throw failure();
  const identity = localTokenIdentity(request.identity);
  const provider = validatedProviderMetadata(request.provider);
  if (
    !provider ||
    !provider.configured ||
    provider.revision < 1 ||
    value.ownerId !== identity.operationId ||
    !providerOperationId(value.credentialRef) ||
    typeof value.ownerSaved !== "boolean" ||
    (value.previewId !== null && !providerOperationId(value.previewId)) ||
    (value.status === "pending" && value.previewId !== null) ||
    (value.status === "committed" &&
      (!value.ownerSaved || value.previewId !== identity.operationId))
  )
    throw failure();
  let service: LocalTokenOperation["request"]["service"] = null;
  if (request.service !== null) {
    const row = request.service;
    if (
      !object(row) ||
      !keys(row, ["id", "versionId", "revision"]) ||
      !providerOperationId(row.id) ||
      !providerOperationId(row.versionId) ||
      !integer(row.revision) ||
      row.revision < 1
    )
      throw failure();
    service = { id: row.id, versionId: row.versionId, revision: row.revision };
  }
  if ((request.purpose === "model_service") !== (service !== null))
    throw failure();
  return {
    version: 1,
    status: value.status as LocalTokenOperation["status"],
    request: {
      identity,
      purpose: request.purpose,
      agentId: request.agentId,
      configurationPath: request.configurationPath,
      configurationPathHash: request.configurationPathHash,
      expectedRevision: request.expectedRevision,
      provider,
      service,
    },
    ownerId: identity.operationId,
    credentialRef: value.credentialRef,
    ownerSaved: value.ownerSaved,
    previewId: value.previewId as string | null,
  };
}
export function localTokenProtocol(value: unknown): LocalTokenProtocol {
  if (
    !object(value) ||
    !keys(value, [
      "version",
      "schemaVersion",
      "storeId",
      "namespaceId",
      "namespaceState",
      "operationLimit",
      "pendingLimit",
      "cancellationLimit",
      "pending",
    ]) ||
    value.version !== 2 ||
    !integer(value.schemaVersion) ||
    value.schemaVersion !== 16 ||
    !providerOperationId(value.storeId) ||
    !providerOperationId(value.namespaceId) ||
    !["active", "unverified"].includes(String(value.namespaceState)) ||
    value.operationLimit !== 4096 ||
    value.pendingLimit !== 4 ||
    value.cancellationLimit !== 4096 ||
    !Array.isArray(value.pending) ||
    value.pending.length > 4 ||
    encoder.encode(JSON.stringify(value)).length > 16 * 1024 * 1024
  )
    throw failure(false);
  const pending = value.pending.map(localTokenOperation),
    ids = new Set<string>();
  for (const row of pending) {
    if (
      row.status !== "pending" ||
      row.request.identity.storeId !== value.storeId ||
      row.request.identity.namespaceId !== value.namespaceId ||
      ids.has(row.ownerId)
    )
      throw failure(false);
    ids.add(row.ownerId);
  }
  return {
    version: 2,
    schemaVersion: value.schemaVersion,
    storeId: value.storeId,
    namespaceId: value.namespaceId,
    namespaceState:
      value.namespaceState as LocalTokenProtocol["namespaceState"],
    operationLimit: 4096,
    pendingLimit: 4,
    cancellationLimit: 4096,
    pending,
  };
}
export function createLocalTokenIdentity(value: unknown): LocalTokenIdentity {
  const protocol = localTokenProtocol(value);
  if (protocol.namespaceState !== "active") throw failure(false);
  return {
    storeId: protocol.storeId,
    namespaceId: protocol.namespaceId,
    operationId: crypto.randomUUID(),
  };
}
export function captureLocalTokenSource(
  value: unknown,
  storeId?: string,
): LocalTokenSource {
  const identity = createLocalTokenIdentity(value);
  if (storeId !== undefined && storeId !== identity.storeId)
    throw failure(false);
  return { identity, epoch: backendReadEpoch(), active: true };
}
export function requireLocalTokenSource(
  source: LocalTokenSource | null | undefined,
  mutation = false,
): LocalTokenSource {
  if (!source)
    throw new DesktopError({
      code: "local_token_origin_missing",
      outcomeUnknown: false,
      message:
        "原入口来源尚未核查，请重新读取配置；旧版后台需要升级后才能准备新的入口。",
    });
  if (!source?.active || source.epoch !== backendReadEpoch())
    throw new DesktopError({
      code: "local_token_source_changed",
      outcomeUnknown: mutation,
      message:
        "原入口的后台连接或界面已变化，请保留原编号并连接原后台核查；重新读取配置后才能准备新的入口。",
    });
  localTokenIdentity(source.identity);
  return source;
}
export function stageLocalToken(
  source: LocalTokenSource | null | undefined,
  fields: Omit<LocalTokenHint, "version" | "identity">,
): LocalTokenHint {
  const origin = requireLocalTokenSource(source),
    hint = { version: 1 as const, identity: origin.identity, ...fields };
  rememberLocalToken(hint);
  requireLocalTokenSource(origin);
  return hint;
}
export function acceptLocalTokenPreview(
  value: unknown,
  source: LocalTokenSource,
  hint: LocalTokenHint,
): ProviderTransaction {
  requireLocalTokenSource(source, true);
  const preview = localTokenPreview(value, source.identity, hint);
  forgetLocalToken(source.identity);
  return preview;
}
function nativeConfig(value: unknown): NativeConfig {
  if (
    !object(value) ||
    !keys(value, [
      "agentId",
      "path",
      "state",
      "revision",
      "mode",
      "endpoint",
      "model",
      "providerName",
      "credential",
      "error",
      "warnings",
    ]) ||
    !agent(value.agentId) ||
    !text(value.path, 16384) ||
    !text(value.state, 100) ||
    (value.revision !== null && !revision(value.revision)) ||
    !text(value.mode, 200) ||
    (value.endpoint !== null && !text(value.endpoint, 2048)) ||
    (value.model !== null && !text(value.model, 800)) ||
    !text(value.providerName, 1000) ||
    !text(value.credential, 1000) ||
    (value.error !== null && !text(value.error, 4000)) ||
    !Array.isArray(value.warnings) ||
    value.warnings.length > 100 ||
    !value.warnings.every((v) => text(v, 4000))
  )
    throw failure();
  return value as unknown as NativeConfig;
}
export function localTokenPreview(
  value: unknown,
  identity: LocalTokenIdentity,
  hint?: LocalTokenHint,
): ProviderTransaction {
  if (
    !object(value) ||
    !keys(value, [
      "localTokenOperation",
      "id",
      "agentId",
      "state",
      "createdAt",
      "appliedAt",
      "before",
      "targetName",
      "provider",
      "model",
      "backupPath",
      "restoreOf",
      "error",
      "routing",
      "activation",
    ]) ||
    value.id !== identity.operationId ||
    !sameLocalTokenIdentity(
      localTokenIdentity(value.localTokenOperation),
      identity,
    ) ||
    !agent(value.agentId) ||
    ![
      "pending",
      "applying",
      "applied",
      "failed",
      "conflict",
      "cancelled",
    ].includes(String(value.state)) ||
    !text(value.createdAt, 64) ||
    !Number.isFinite(Date.parse(value.createdAt)) ||
    (value.appliedAt !== null &&
      (!text(value.appliedAt, 64) ||
        !Number.isFinite(Date.parse(value.appliedAt)))) ||
    !text(value.targetName, 2000) ||
    !text(value.backupPath, 16384) ||
    value.restoreOf !== null ||
    (value.error !== null && !text(value.error, 4000)) ||
    (value.model != null && !text(value.model, 800))
  )
    throw failure();
  const before = nativeConfig(value.before),
    provider = validatedProviderMetadata(value.provider),
    routing = value.routing;
  if (
    !provider ||
    !object(routing) ||
    !keys(routing, [
      "id",
      "clientProtocol",
      "upstreamProtocol",
      "localEndpoint",
      "models",
    ]) ||
    !providerOperationId(routing.id) ||
    !text(routing.clientProtocol, 32) ||
    !text(routing.upstreamProtocol, 32) ||
    !text(routing.localEndpoint, 2048) ||
    (routing.models !== undefined &&
      (!Array.isArray(routing.models) ||
        routing.models.length > 2000 ||
        !routing.models.every((v) => text(v, 800))))
  )
    throw failure();
  if (
    hint &&
    (value.agentId !== hint.agentId ||
      (before.revision ?? "absent") !== hint.expectedRevision ||
      provider.id !== hint.providerId ||
      (hint.providerRevision !== undefined &&
        provider.revision !== hint.providerRevision) ||
      (hint.model !== undefined && value.model !== hint.model))
  )
    throw failure();
  if (
    value.activation != null &&
    (!object(value.activation) ||
      !keys(value.activation, ["state", "updatedAt"]) ||
      !["deferred", "awaiting_restart", "launch_requested"].includes(
        String(value.activation.state),
      ) ||
      !text(value.activation.updatedAt, 64))
  )
    throw failure();
  return { ...(value as unknown as ProviderTransaction), provider, before };
}
export function localTokenResult(
  value: unknown,
  identity: LocalTokenIdentity,
  hint?: LocalTokenHint,
): LocalTokenResult {
  if (
    !object(value) ||
    !keys(value, [
      "version",
      "identity",
      "operation",
      "credential",
      "preview",
      "cancellation",
      "canContinue",
      "sourceMessage",
    ]) ||
    value.version !== 2 ||
    !sameLocalTokenIdentity(localTokenIdentity(value.identity), identity) ||
    encoder.encode(JSON.stringify(value)).length > 16 * 1024 * 1024
  )
    throw failure();
  const operation =
    value.operation === null ? null : localTokenOperation(value.operation);
  if (
    operation &&
    !sameLocalTokenIdentity(operation.request.identity, identity)
  )
    throw failure();
  if (
    operation &&
    hint &&
    (operation.request.purpose !== hint.purpose ||
      operation.request.agentId !== hint.agentId ||
      operation.request.expectedRevision !== hint.expectedRevision ||
      operation.request.provider.id !== hint.providerId ||
      (hint.providerRevision !== undefined &&
        operation.request.provider.revision !== hint.providerRevision) ||
      (hint.model !== undefined &&
        operation.request.provider.model !== hint.model) ||
      (hint.serviceId !== undefined &&
        operation.request.service?.id !== hint.serviceId) ||
      (hint.serviceRevision !== undefined &&
        operation.request.service?.revision !== hint.serviceRevision))
  )
    throw failure();
  const preview =
    value.preview === null
      ? null
      : localTokenPreview(value.preview, identity, hint);
  let cancellation: LocalTokenResult["cancellation"] = null;
  if (value.cancellation !== null) {
    const row = value.cancellation;
    if (
      !object(row) ||
      !keys(row, ["version", "identity"]) ||
      row.version !== 1 ||
      !sameLocalTokenIdentity(localTokenIdentity(row.identity), identity)
    )
      throw failure();
    cancellation = { version: 1, identity: localTokenIdentity(row.identity) };
  }
  if (
    (!operation && !preview && !cancellation) ||
    (cancellation &&
      (operation !== null ||
        preview !== null ||
        value.credential != null ||
        value.canContinue === true)) ||
    (operation?.status === "committed" &&
      preview &&
      operation.previewId !== preview.id)
  )
    throw failure();
  const credential =
    value.credential == null
      ? null
      : credentialOperation(value.credential, identity.operationId);
  if (
    (credential && !operation) ||
    (credential?.receipt &&
      credential.receipt.credentialRef !== operation?.credentialRef) ||
    (value.canContinue !== undefined &&
      typeof value.canContinue !== "boolean") ||
    (value.sourceMessage !== undefined && !text(value.sourceMessage, 4000)) ||
    (value.canContinue === true &&
      (!operation ||
        operation.status !== "pending" ||
        preview !== null ||
        (credential !== null &&
          !["pending", "ready"].includes(credential.status))))
  )
    throw failure();
  return {
    version: 2,
    identity,
    operation,
    credential,
    preview,
    cancellation,
    canContinue: value.canContinue === true,
    sourceMessage:
      typeof value.sourceMessage === "string"
        ? value.sourceMessage
        : "原结果已核查。",
  };
}

export const LOCAL_TOKEN_CHANGED = "local-agent-local-token-changed";
export const LOCAL_TOKEN_CANCELLED = "local-agent-local-token-cancelled";
const PREFIX = "local-agent.local-token.v1.";
function key(identity: LocalTokenIdentity) {
  return (
    PREFIX +
    identity.storeId +
    "." +
    identity.namespaceId +
    "." +
    identity.operationId
  );
}
function parseHint(value: unknown): LocalTokenHint {
  if (
    !object(value) ||
    !keys(value, [
      "version",
      "identity",
      "purpose",
      "agentId",
      "expectedRevision",
      "providerId",
      "providerRevision",
      "model",
      "serviceId",
      "serviceRevision",
    ]) ||
    value.version !== 1 ||
    !purpose(value.purpose) ||
    !agent(value.agentId) ||
    !revision(value.expectedRevision) ||
    !text(value.providerId, 100) ||
    !value.providerId ||
    (value.providerRevision !== undefined &&
      !integer(value.providerRevision)) ||
    (value.model !== undefined && !text(value.model, 800)) ||
    (value.serviceId !== undefined && !providerOperationId(value.serviceId)) ||
    (value.serviceRevision !== undefined && !integer(value.serviceRevision))
  )
    throw failure(false);
  return {
    ...(value as unknown as LocalTokenHint),
    identity: localTokenIdentity(value.identity),
  };
}
export function localTokenRecovery(): {
  hints: LocalTokenHint[];
  invalid: boolean;
} {
  const hints: LocalTokenHint[] = [];
  let invalid = false;
  try {
    const owned: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const name = localStorage.key(i);
      if (name?.startsWith(PREFIX)) owned.push(name);
    }
    if (owned.length > 4) return { hints, invalid: true };
    for (const name of owned) {
      const raw = localStorage.getItem(name);
      try {
        if (!raw || encoder.encode(raw).length > 4096) throw failure(false);
        const hint = parseHint(JSON.parse(raw));
        if (key(hint.identity) !== name) throw failure(false);
        hints.push(hint);
      } catch {
        invalid = true;
      }
    }
  } catch {
    invalid = true;
  }
  return { hints, invalid };
}
export function rememberLocalToken(input: LocalTokenHint): void {
  const hint = parseHint(input),
    recovery = localTokenRecovery(),
    name = key(hint.identity),
    raw = JSON.stringify(hint),
    old = localStorage.getItem(name);
  if (
    recovery.invalid ||
    encoder.encode(raw).length > 4096 ||
    (old !== null && old !== raw) ||
    (old === null && recovery.hints.length >= 4)
  )
    throw failure(false);
  localStorage.setItem(name, raw);
  if (localStorage.getItem(name) !== raw) throw failure(false);
  window.dispatchEvent(new Event(LOCAL_TOKEN_CHANGED));
}
export function forgetLocalToken(identity: LocalTokenIdentity): void {
  const name = key(identity),
    raw = localStorage.getItem(name);
  if (raw === null) return;
  const hint = parseHint(JSON.parse(raw));
  if (!sameLocalTokenIdentity(hint.identity, identity)) throw failure();
  localStorage.removeItem(name);
  if (localStorage.getItem(name) !== null) throw failure();
  window.dispatchEvent(new Event(LOCAL_TOKEN_CHANGED));
}
