import type {
  DynamicBinding,
  DynamicChange,
  DynamicVersion,
  NativeConfig,
  Provider,
  ProviderImportCandidate,
  ProviderStatus,
  ProviderTransaction,
  ProviderVerification,
} from "./types";
import {
  localTokenIdentity,
  localTokenProtocol,
} from "./local-token-operation";
import {
  type Decoder,
  array,
  boolean,
  count,
  date,
  nonempty,
  nullable,
  oneOf,
  optional,
  record,
  response,
  shape,
  text,
  url,
} from "./response-values";

export const providerValue: Decoder<Provider> = shape({
  id: nonempty(100),
  name: text(400),
  vendor: optional(text(50)),
  endpoint: text(2048),
  protocol: oneOf("responses", "messages", "chat-completions"),
  model: text(800),
  configured: boolean,
  credentialRef: nullable(nonempty()),
  credentialLabel: nullable(text(200)),
  models: array(nonempty(800), 2000),
  disabledModels: optional(array(nonempty(800), 2000)),
  revision: count,
  category: oneOf("cloud", "custom", "local"),
  modelCatalog: optional(
    nullable(
      shape({
        fetchedAt: date,
        sourceUrl: url,
        endpoint: url,
        protocol: nonempty(32),
        credentialRef: nullable(nonempty()),
      }),
    ),
  ),
});
const nativeConfig: Decoder<NativeConfig> = shape({
  agentId: nonempty(),
  path: text(),
  state: nonempty(),
  revision: nullable(text()),
  mode: nonempty(),
  endpoint: nullable(text(2048)),
  model: nullable(text(800)),
  providerName: text(),
  credential: text(),
  error: nullable(text()),
  warnings: array(text(), 1000),
});
const transaction: Decoder<ProviderTransaction> = shape({
  localTokenOperation: optional(localTokenIdentity),
  id: nonempty(),
  agentId: nonempty(),
  state: nonempty(),
  createdAt: date,
  appliedAt: nullable(date),
  before: nativeConfig,
  targetName: text(),
  provider: nullable(providerValue),
  model: optional(nullable(text(800))),
  backupPath: text(),
  restoreOf: nullable(text()),
  error: nullable(text()),
  activation: optional(
    nullable(
      shape({
        state: oneOf("deferred", "awaiting_restart", "launch_requested"),
        updatedAt: date,
      }),
    ),
  ),
  routing: optional(
    shape({
      id: nonempty(),
      clientProtocol: nonempty(32),
      upstreamProtocol: nonempty(32),
      localEndpoint: url,
      models: optional(array(nonempty(800), 2000)),
    }),
  ),
});
const verification: Decoder<ProviderVerification> = shape({
  providerId: nonempty(),
  model: optional(nullable(text(800))),
  fingerprint: text(),
  state: nonempty(),
  checkedAt: date,
  message: text(),
});
const version: Decoder<DynamicVersion> = shape({
  id: nonempty(),
  bindingId: nonempty(),
  provider: providerValue,
  createdAt: date,
});
const binding: Decoder<DynamicBinding> = shape({
  id: nonempty(),
  agentId: nonempty(),
  scope: text(),
  localEndpoint: url,
  modelService: optional(boolean),
  lastObserved: optional(
    shape({
      providerName: text(),
      model: nullable(text(800)),
      status: nonempty(),
      routeVersionId: nonempty(),
      startedAt: date,
    }),
  ),
  credentialRef: nonempty(),
  generation: nonempty(),
  target: version,
  createdAt: date,
});
const change: Decoder<DynamicChange> = shape({
  id: nonempty(),
  bindingId: nonempty(),
  expectedGeneration: nonempty(),
  state: oneOf("pending", "applied", "cancelled"),
  before: version,
  target: providerValue,
  published: nullable(version),
  createdAt: date,
});
const status: Decoder<ProviderStatus> = shape({
  localTokenProtocol: optional((value) => {
    // An unsupported optional capability stays unavailable on older services.
    const row = record(value);
    count(row.version);
    return row.version === 2 ? localTokenProtocol(value) : undefined;
  }),
  configs: array(nativeConfig),
  transactions: array(transaction),
  backupProtection: nonempty(),
  verifications: array(verification),
  runtimes: array(
    shape({
      agentId: nonempty(),
      version: nullable(text()),
      supported: boolean,
      processes: nullable(count),
      reason: text(),
    }),
  ),
  dynamicBindings: optional(array(binding)),
  dynamicChanges: optional(array(change)),
  dynamicLeases: optional(
    array(
      shape({
        bindingId: nonempty(),
        routeVersionId: nonempty(),
        turns: count,
      }),
    ),
  ),
});
const candidate: Decoder<ProviderImportCandidate> = shape({
  id: nonempty(),
  agentId: oneOf("claude", "codex"),
  sourcePath: text(),
  sourceLabel: text(),
  revision: nonempty(),
  name: text(),
  vendor: nonempty(),
  endpoint: nullable(text(2048)),
  protocol: nonempty(),
  model: nullable(text(800)),
  active: boolean,
  credentialSource: text(),
  canImport: boolean,
  reason: nullable(text()),
  existingProviderId: nullable(text()),
});
export const providerStatus = (value: unknown) =>
  response(status, value, "供应商状态");
export function providerTransaction(
  value: unknown,
  expected: Partial<
    Pick<ProviderTransaction, "id" | "agentId" | "restoreOf">
  > & { beforeRevision?: string } = {},
) {
  return response(
    (input) => {
      const result = transaction(input);
      const { beforeRevision, ...identity } = expected;
      if (
        result.before.agentId !== result.agentId ||
        Object.entries(identity).some(
          ([key, value]) => result[key as keyof typeof identity] !== value,
        ) ||
        (beforeRevision !== undefined &&
          result.before.revision !== beforeRevision)
      )
        throw Error("transaction identity mismatch");
      return result;
    },
    value,
    "配置操作",
    true,
  );
}
export function providerDynamicChange(
  value: unknown,
  expected: Partial<
    Pick<DynamicChange, "id" | "bindingId" | "expectedGeneration">
  > & { target?: Pick<Provider, "id" | "revision" | "model"> } = {},
) {
  return response(
    (input) => {
      const result = change(input);
      const { target, ...identity } = expected;
      if (
        result.before.bindingId !== result.bindingId ||
        (result.published && result.published.bindingId !== result.bindingId) ||
        Object.entries(identity).some(
          ([key, value]) => result[key as keyof typeof identity] !== value,
        ) ||
        (target &&
          (result.target.id !== target.id ||
            result.target.revision !== target.revision ||
            result.target.model !== target.model))
      )
        throw Error("dynamic change identity mismatch");
      return result;
    },
    value,
    "动态路由操作",
    true,
  );
}
export const providerImportCandidates = (value: unknown) =>
  response(array(candidate), value, "本机导入候选");
export const providerVerification = (value: unknown) =>
  response(verification, value, "模型验证");
