import catalogSchema from "../../../contracts/plugins/builtin/catalog.schema.json";
import changeSchema from "../../../contracts/plugins/builtin/change.schema.json";
import replySchema from "../../../contracts/plugins/builtin/reply.schema.json";
import type {
  CompositionCatalog,
  CompositionChange,
  CompositionOperation,
  CompositionReply,
} from "../../../contracts/plugins/builtin/types";
import { parseSchema } from "./contract";

const uuid =
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const identifier = /^[A-Za-z0-9._-]+$/;
function invalid(): never {
  throw new Error("插件状态响应身份或版本不一致，请保留原操作并重新核查。");
}
function decimal(value: string, max = 9007199254740991n) {
  return /^(0|[1-9][0-9]{0,19})$/.test(value) && BigInt(value) <= max;
}
export function builtinCatalog(value: unknown): CompositionCatalog {
  const catalog = parseSchema<CompositionCatalog>(catalogSchema, value);
  if (
    catalog.schemaVersion !== 1 ||
    !uuid.test(catalog.storeId) ||
    !uuid.test(catalog.host.generation) ||
    !decimal(catalog.revision) ||
    catalog.host.plugins.length > 256
  )
    invalid();
  const ids = new Set<string>();
  const addresses = new Set<string>();
  for (const plugin of catalog.host.plugins) {
    if (
      !identifier.test(plugin.id) ||
      plugin.id.length > 100 ||
      ids.has(plugin.id) ||
      plugin.executionKind !== "builtin" ||
      !plugin.displayName ||
      plugin.displayName.length > 200 ||
      !plugin.version ||
      plugin.version.length > 100 ||
      !decimal(plugin.activationEpoch, 18446744073709551615n) ||
      (plugin.runtimeActive && !plugin.desiredEnabled) ||
      plugin.capabilities.length > 64 ||
      plugin.requires.length > 64 ||
      plugin.optionalRequires.length > 64
    )
      invalid();
    ids.add(plugin.id);
    for (const capability of plugin.capabilities) {
      const key = capability.address;
      const address = `${key.name}@${key.version}:${key.instance}`;
      if (
        !identifier.test(key.name) ||
        key.name.length > 128 ||
        !identifier.test(key.instance) ||
        key.instance.length > 128 ||
        addresses.has(address) ||
        !Number.isInteger(key.version) ||
        key.version < 1 ||
        key.version > 65535
      )
        invalid();
      addresses.add(address);
    }
    if (
      plugin.resources.some(
        (resource) =>
          !Number.isSafeInteger(resource.count) ||
          resource.count < 0 ||
          resource.count > 256,
      )
    )
      invalid();
  }
  return catalog;
}
export function builtinChange(value: unknown): CompositionChange {
  const input = parseSchema<CompositionChange>(changeSchema, value);
  const ids = input.composition.disabledPluginIds;
  const removed = input.composition.removedPluginIds ?? [];
  if (
    !uuid.test(input.operationId) ||
    !uuid.test(input.storeId) ||
    !uuid.test(input.expectedGeneration) ||
    !decimal(input.expectedRevision) ||
    [ids, removed].some(
      (list) =>
        list.length > 256 ||
        list.some(
          (id, index) =>
            !identifier.test(id) ||
            id.length > 100 ||
            (index > 0 && list[index - 1] >= id),
        ),
    )
  )
    invalid();
  const request = {
    ...input,
    composition: {
      ...input.composition,
      disabledPluginIds: [...ids],
      ...(input.composition.removedPluginIds
        ? { removedPluginIds: [...removed] }
        : {}),
    },
  };
  Object.freeze(request.composition.disabledPluginIds);
  if (request.composition.removedPluginIds)
    Object.freeze(request.composition.removedPluginIds);
  if (request.expectedImpact) {
    Object.freeze(request.expectedImpact.modelServices);
    Object.freeze(request.expectedImpact);
  }
  Object.freeze(request.composition);
  Object.freeze(request);
  return request;
}
export function builtinReply(
  value: unknown,
  request: CompositionChange | CompositionOperation,
): CompositionReply {
  const reply = parseSchema<CompositionReply>(replySchema, value);
  builtinCatalog(reply.catalog);
  if (reply.catalog.storeId !== request.storeId) invalid();
  if (reply.receipt) {
    const receipt = reply.receipt;
    if (
      receipt.storeId !== request.storeId ||
      receipt.operationId !== request.operationId ||
      !decimal(receipt.revision)
    )
      invalid();
    if (BigInt(receipt.revision) > BigInt(reply.catalog.revision)) invalid();
    if (
      "composition" in request &&
      (BigInt(receipt.revision) !== BigInt(request.expectedRevision) + 1n ||
        receipt.composition.profile !== request.composition.profile ||
        receipt.composition.disabledPluginIds.length !==
          request.composition.disabledPluginIds.length ||
        receipt.composition.disabledPluginIds.some(
          (id, index) => id !== request.composition.disabledPluginIds[index],
        ) ||
        JSON.stringify(receipt.composition.removedPluginIds ?? []) !==
          JSON.stringify(request.composition.removedPluginIds ?? []))
    )
      invalid();
  }
  return reply;
}
