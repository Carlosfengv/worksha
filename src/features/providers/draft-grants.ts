import type {
  DraftGrantOwner,
  DraftGrantScope,
} from "../../../contracts/domain/provider-draft-grant/types";
import scopeSchema from "../../../contracts/domain/provider-draft-grant/scope.schema.json";
import selectionSchema from "../../../contracts/domain/provider-connection/selection.schema.json";
import { acceptsSchema } from "@/shared/lib/contract-schema";
import {
  recoveryKeys,
  writeRecoveryRecord,
  removeRecoveryRecord,
} from "@/shared/lib/recovery-storage";
import { providerDraftGrantApi } from "./api";
import {
  isExplicitProviderSource,
  sameProviderCatalog,
  type ProviderCatalogSelection,
  type ProviderWriteRoute,
} from "./catalog-operation";
import { providerRecoveryRecords } from "./pending-operations";
import type {
  ProviderOperation,
  ProviderWriteRequest,
} from "./operation-receipt";

const PREFIX = "local-agent.provider-draft-owner.v1.";
const LIMIT = 128;
const BYTES = 512 * 1024;
interface LocalOwner {
  version: 1;
  owner: DraftGrantOwner;
  editorId: string;
  providerId: string;
  scope: DraftGrantScope;
  selection: ProviderCatalogSelection | null;
  operationId: string | null;
  attached: boolean;
  closed: boolean;
  generation?: number;
}
const id = (s: unknown): s is string =>
  typeof s === "string" &&
  /^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(s);
const key = (r: LocalOwner) => PREFIX + r.owner.ownerId;
function catalog(route: ProviderWriteRoute | null) {
  return route && isExplicitProviderSource(route)
    ? route.source.kind === "external"
      ? route.source.catalog
      : null
    : route;
}
function matchesScope(s: ProviderCatalogSelection, scope: DraftGrantScope) {
  const t = s.target;
  return (
    t.kind === "draft" &&
    s.pluginId === scope.catalog.pluginId &&
    s.digest === scope.catalog.digest &&
    t.endpoint === scope.draft.endpoint &&
    t.protocol === scope.draft.protocol &&
    t.vendor === scope.draft.vendor &&
    (t.credentialRef ?? null) === (scope.draft.credentialRef ?? null)
  );
}
function records(): { record: LocalOwner; raw: string }[] {
  let bytes = 0;
  const rows = [];
  for (const k of recoveryKeys(PREFIX)) {
    const raw = localStorage.getItem(k) ?? "";
    bytes += new TextEncoder().encode(raw).length;
    if (rows.length >= LIMIT || bytes > BYTES)
      throw Error("草稿授权恢复记录超过预算，请在插件管理中核查；未自动移除。");
    let r: LocalOwner;
    try {
      r = JSON.parse(raw) as LocalOwner;
    } catch {
      throw Error("草稿授权恢复记录无法读取，已保留原记录。");
    }
    if (
      !r ||
      r.version !== 1 ||
      ![9, 10].includes(Object.keys(r).length) ||
      Object.keys(r).some(
        (k) =>
          ![
            "version",
            "owner",
            "editorId",
            "providerId",
            "scope",
            "selection",
            "operationId",
            "attached",
            "closed",
            "generation",
          ].includes(k),
      ) ||
      !r.owner ||
      Object.keys(r.owner).length !== 2 ||
      !id(r.owner.storeId) ||
      !id(r.owner.ownerId) ||
      k !== key(r) ||
      !id(r.editorId) ||
      typeof r.providerId !== "string" ||
      !r.providerId ||
      r.providerId.length > 256 ||
      typeof r.closed !== "boolean" ||
      typeof r.attached !== "boolean" ||
      (r.generation !== undefined &&
        (!Number.isSafeInteger(r.generation) || r.generation < 0)) ||
      (r.operationId !== null && !id(r.operationId)) ||
      !acceptsSchema(scopeSchema, r.scope) ||
      (r.selection !== null &&
        (!acceptsSchema(selectionSchema, r.selection) ||
          !matchesScope(r.selection, r.scope)))
    )
      throw Error("草稿授权恢复身份无法核查，已保留原记录。");
    rows.push({ record: r, raw });
  }
  return rows;
}
function update(r: LocalOwner, mutate: (current: LocalOwner) => void) {
  const current = records().find(
    (v) => v.record.owner.ownerId === r.owner.ownerId,
  );
  if (!current || current.record.owner.storeId !== r.owner.storeId)
    throw Error("草稿授权恢复记录已变化。");
  const next = structuredClone(current.record);
  mutate(next);
  const raw = JSON.stringify(next);
  if (localStorage.getItem(key(r)) !== current.raw)
    throw Error("草稿授权恢复记录已变化。");
  localStorage.setItem(key(r), raw);
  if (localStorage.getItem(key(r)) !== raw)
    throw Error("草稿授权恢复记录未能完整保存。");
  return next;
}
async function release(row: ReturnType<typeof records>[number]) {
  const result = await providerDraftGrantApi.perform({
    kind: "release",
    owner: row.record.owner,
  });
  if (result.state === "released")
    removeRecoveryRecord(key(row.record), (raw) => raw === row.raw);
}
const sweeps = new Map<string, Promise<void>>();
export function sweepClosedDraftGrants(storeId: string) {
  const active = sweeps.get(storeId);
  if (active) return active;
  const work = (async () => {
    const pending = providerRecoveryRecords();
    if (pending.overflow || pending.records.some((r) => !r.request)) return;
    for (const row of records()) {
      const r = row.record;
      if (r.owner.storeId !== storeId || (!r.closed && !r.operationId))
        continue;
      // Closing an unresolved editor retains its original authority. Only a
      // confirmed terminal reply may finalize it while that hint still exists.
      if (
        pending.records.some(
          (v) =>
            v.request?.storeId === storeId &&
            v.request.provider.id === r.providerId,
        )
      )
        continue;
      await release(row);
    }
  })().finally(() => {
    if (sweeps.get(storeId) === work) sweeps.delete(storeId);
  });
  sweeps.set(storeId, work);
  return work;
}
export async function closeEditorDraftGrants(
  editorId: string,
  storeId: string,
) {
  const pending = providerRecoveryRecords();
  for (const row of records()) {
    if (
      row.record.editorId !== editorId ||
      row.record.owner.storeId !== storeId
    )
      continue;
    try {
      update(row.record, (r) => {
        r.closed = true;
      });
    } catch {
      // Quota failure must not strand a known idle exclusive grant. Unknown
      // requests keep authority; the host also checks its original SQL pin.
      if (
        !pending.overflow &&
        pending.records.every((v) => !!v.request) &&
        !pending.records.some(
          (v) =>
            v.request?.storeId === storeId &&
            v.request.provider.id === row.record.providerId,
        )
      )
        await release(row);
    }
  }
  await sweepClosedDraftGrants(storeId);
}
export async function acquireEditorDraftGrant(
  editorId: string,
  providerId: string,
  storeId: string,
  scope: DraftGrantScope,
  generation: number,
) {
  const scopeKey = (s: DraftGrantScope) =>
    JSON.stringify([
      s.catalog.pluginId,
      s.catalog.digest,
      s.draft.endpoint,
      s.draft.protocol,
      s.draft.vendor,
      s.draft.credentialRef ?? null,
      s.label,
      s.allowPrivateNetwork,
    ]);
  const same = (s: DraftGrantScope) => scopeKey(s) === scopeKey(scope);
  let current: LocalOwner | undefined;
  for (const row of records()) {
    const r = row.record;
    if (
      r.owner.storeId !== storeId ||
      r.editorId !== editorId ||
      r.closed ||
      r.operationId
    )
      continue;
    if (same(r.scope) && r.generation === generation) current = r;
    else
      update(r, (v) => {
        v.closed = true;
      });
  }
  await sweepClosedDraftGrants(storeId);
  if (!current) {
    const rows = records();
    if (
      rows.length >= LIMIT ||
      rows.reduce((sum, r) => sum + new TextEncoder().encode(r.raw).length, 0) >
        BYTES - 16384
    )
      throw Error("草稿授权恢复记录已达预算，请先核查已有授权。");
    const prepared = await providerDraftGrantApi.perform({
      kind: "open",
      storeId,
      scope,
    });
    current = {
      version: 1,
      owner: prepared.owner,
      editorId,
      providerId,
      scope,
      selection: null,
      operationId: null,
      attached: false,
      closed: false,
      generation,
    };
    try {
      writeRecoveryRecord(key(current), JSON.stringify(current), 16384);
    } catch (e) {
      // Open created no authority. No grant may be acquired without this record.
      await providerDraftGrantApi
        .perform({ kind: "release", owner: current.owner })
        .catch(() => null);
      throw e;
    }
  }
  const result = await providerDraftGrantApi.perform({
    kind: "acquire",
    owner: current.owner,
  });
  if (!result.selection || !matchesScope(result.selection, scope))
    throw Error("草稿授权返回的范围无法核查。");
  const selected = result.selection;
  update(current, (r) => {
    r.selection = selected;
  });
  return selected;
}
export function reserveDraftGrantOperation(
  request: ProviderWriteRequest,
  route: ProviderWriteRoute | null,
) {
  const selected = catalog(route);
  if (!selected || !route || !isExplicitProviderSource(route)) return;
  const row = records().find(
    ({ record: r }) =>
      r.owner.storeId === request.storeId &&
      r.providerId === request.provider.id &&
      sameProviderCatalog(r.selection, selected),
  );
  if (!row) return; // Old/preexisting grants have no ownership proof; never adopt them.
  return update(row.record, (v) => {
    if (v.operationId && v.operationId !== request.operationId)
      throw Error("草稿授权属于另一个原操作。");
    v.operationId = request.operationId;
  });
}
export function hasDraftGrantOperation(
  request: ProviderWriteRequest,
  route: ProviderWriteRoute | null,
) {
  const selected = catalog(route);
  if (!selected || !route || !isExplicitProviderSource(route)) return false;
  return records().some(
    ({ record: r }) =>
      r.owner.storeId === request.storeId &&
      r.providerId === request.provider.id &&
      r.operationId === request.operationId &&
      sameProviderCatalog(r.selection, selected),
  );
}
export async function attachDraftGrantOperation(
  request: ProviderWriteRequest,
  route: ProviderWriteRoute | null,
) {
  const r = reserveDraftGrantOperation(request, route);
  if (!r || !route || !isExplicitProviderSource(route)) return;
  await providerDraftGrantApi.perform({
    kind: "attach",
    owner: r.owner,
    request: { operation: request, ...route },
  });
  update(r, (v) => {
    v.attached = true;
  });
}
export async function finishDraftGrantOperation(
  operation: ProviderOperation,
  route: ProviderWriteRoute | null,
) {
  if (operation.status === "pending") return;
  const request = operation.request,
    selected = catalog(route);
  if (!selected) return;
  for (const row of records()) {
    const r = row.record;
    if (
      r.owner.storeId !== request.storeId ||
      r.providerId !== request.provider.id ||
      r.operationId !== request.operationId ||
      !sameProviderCatalog(r.selection, selected)
    )
      continue;
    let closed: LocalOwner;
    try {
      closed = update(r, (v) => {
        v.closed = true;
      });
    } catch {
      await release(row);
      continue;
    }
    // The host verifies its immutable request pin and the actual SQL terminal.
    await release({ record: closed, raw: JSON.stringify(closed) });
  }
  await sweepClosedDraftGrants(request.storeId);
}
