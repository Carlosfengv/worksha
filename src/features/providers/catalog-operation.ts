import type { Selection } from "../../../contracts/domain/provider-connection/types";
import selectionSchema from "../../../contracts/domain/provider-connection/selection.schema.json" with { type: "json" };
import { acceptsSchema } from "@/shared/lib/contract-schema";
import {
  storedProviderRequest,
  type ProviderWriteRequest,
} from "./operation-receipt";

export type ProviderCatalogSelection = Selection;
export interface ExplicitProviderSource {
  source: import("../../../contracts/domain/provider-source-operation/types").SourceChoice;
  expectedSourceRevision: string | null;
}
export type ProviderWriteRoute = Selection | ExplicitProviderSource;
export function isExplicitProviderSource(
  value: ProviderWriteRoute,
): value is ExplicitProviderSource {
  return Object.hasOwn(value, "source");
}
export function selectedProviderRoute(
  value: unknown,
  request: ProviderWriteRequest,
): ProviderWriteRoute | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  if (!Object.hasOwn(value, "source"))
    return selectedProviderCatalog(value, request);
  const route = value as ExplicitProviderSource;
  if (
    Object.keys(value).length !== 2 ||
    !Object.hasOwn(value, "expectedSourceRevision") ||
    (route.expectedSourceRevision !== null &&
      (typeof route.expectedSourceRevision !== "string" ||
        !/^[a-f0-9]{64}$/.test(route.expectedSourceRevision)))
  )
    return null;
  const source = route.source;
  if (!source || typeof source !== "object" || Array.isArray(source))
    return null;
  if (source.kind === "builtin" && Object.keys(source).length === 1)
    return structuredClone(route);
  if (
    source.kind === "external" &&
    Object.keys(source).length === 2 &&
    selectedProviderCatalog(source.catalog, request)
  )
    return structuredClone(route);
  return null;
}
export function sameProviderRoute(
  a: ProviderWriteRoute | null | undefined,
  b: ProviderWriteRoute | null | undefined,
) {
  if (!a || !b) return !a && !b;
  if (isExplicitProviderSource(a) || isExplicitProviderSource(b)) {
    if (!isExplicitProviderSource(a) || !isExplicitProviderSource(b))
      return false;
    return (
      a.expectedSourceRevision === b.expectedSourceRevision &&
      a.source.kind === b.source.kind &&
      sameProviderCatalog(
        a.source.kind === "external" ? a.source.catalog : null,
        b.source.kind === "external" ? b.source.catalog : null,
      )
    );
  }
  return sameProviderCatalog(a, b);
}
export function selectedProviderCatalog(
  value: unknown,
  request: ProviderWriteRequest,
): Selection | null {
  if (!acceptsSchema(selectionSchema, value)) return null;
  const selection = value as Selection;
  if (
    !selection.pluginId ||
    selection.pluginId.length > 128 ||
    !/^[a-f0-9]{64}$/.test(selection.digest) ||
    !selection.bindingId ||
    selection.bindingId.length > 512
  )
    return null;
  const target = selection.target,
    p = request.provider;
  if (target.kind === "saved") {
    if (
      target.providerId !== p.id ||
      target.expectedRevision !== String(p.revision)
    )
      return null;
  } else if (
    target.endpoint !== p.endpoint ||
    target.protocol !== p.protocol ||
    target.vendor !== p.vendor ||
    (target.credentialRef ?? null) !== p.credentialRef ||
    !target.label.trim() ||
    Array.from(target.label).length > 120
  )
    return null;
  return structuredClone(selection);
}
export function sameProviderCatalog(
  a: Selection | null | undefined,
  b: Selection | null | undefined,
) {
  const key = (s: Selection | null | undefined) =>
    s
      ? JSON.stringify([
          s.pluginId,
          s.digest,
          s.bindingId,
          s.target.kind,
          s.target.kind === "saved"
            ? [s.target.providerId, s.target.expectedRevision]
            : [
                s.target.label,
                s.target.endpoint,
                s.target.protocol,
                s.target.vendor,
                s.target.credentialRef ?? null,
              ],
        ])
      : null;
  return key(a) === key(b);
}
export function storedProviderSubmission(value: unknown): {
  request: ProviderWriteRequest;
  catalog: ProviderWriteRoute | null;
} | null {
  const native = storedProviderRequest(value);
  if (native) return { request: native, catalog: null };
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const envelope = value as Record<string, unknown>;
  if (
    envelope.version === 3 &&
    Object.keys(envelope).length === 3 &&
    Object.keys(envelope).every((key) =>
      ["version", "request", "route"].includes(key),
    )
  ) {
    const request = storedProviderRequest(envelope.request);
    const route = request
      ? selectedProviderRoute(envelope.route, request)
      : null;
    return request && route && isExplicitProviderSource(route)
      ? { request, catalog: route }
      : null;
  }
  if (
    envelope.version !== 2 ||
    Object.keys(envelope).length !== 3 ||
    !Object.keys(envelope).every((key) =>
      ["version", "request", "catalog"].includes(key),
    )
  )
    return null;
  const request = storedProviderRequest(envelope.request);
  if (!request) return null;
  const catalog = selectedProviderCatalog(envelope.catalog, request);
  return catalog ? { request, catalog } : null;
}
