import { useCallback, useReducer, useRef, useState } from "react";
import { providersApi } from "../api";
import { isLoopbackEndpoint, providerCatalog } from "../catalog";
import { message } from "../display";
import { providerEditorDraft, reduceProviderDraft } from "../editor-draft";
import { catalogKey, providerCatalogSource } from "../model-inventory";
import { validateCredentialInput } from "../credential-input";
import type { Provider } from "../types";
import type { RecoverableCredentialReceipt } from "../credential-receipt";
import { useCatalogContext, type CatalogContext } from "./use-catalog-context";
import { useCredentialOperations } from "./use-credential-operations";
import { useProviderWrite } from "./use-provider-write";

import { useEditorCatalogSource } from "./use-editor-catalog-source";
import { useEditorCatalogCache } from "./use-editor-catalog-cache";
import { useProviderPresets } from "./use-provider-presets";
import type { PresetAdvice } from "../preset-advice";

type Intent = "models" | "save";
type Preparation =
  | { kind: "idle" }
  | { kind: "reading" | "preparing-save"; source: CatalogContext };
export interface ProviderEditorProps {
  provider: Provider;
  storeId?: string;
  onClose: () => void;
  onSaved: () => Promise<void>;
}

export function useProviderEditor({
  provider,
  storeId,
  onSaved,
}: ProviderEditorProps) {
  const [originStoreId] = useState(storeId);
  const [form, dispatch] = useReducer(
    reduceProviderDraft,
    provider,
    providerEditorDraft,
  );
  const draft = form.provider,
    automatic = form.mode === "automatic";
  const context = useCatalogContext(storeId);
  const catalogSource = useEditorCatalogSource(
    provider,
    originStoreId === storeId ? storeId : undefined,
  );
  const credentials = useCredentialOperations(originStoreId, storeId ?? null);
  const keyInput = useRef<HTMLInputElement>(null);
  const [hasNewKey, setHasNewKey] = useState(false);
  const working = useRef<{ token: symbol; source: CatalogContext } | null>(
    null,
  );
  const [preparation, setPreparation] = useState<Preparation>({ kind: "idle" });
  const catalogCache = useEditorCatalogCache(
    originStoreId === storeId ? storeId : undefined,
    catalogKey(draft),
    catalogSource.generation,
  );
  const [modelError, setModelError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [viewError, setViewError] = useState<string | null>(null);
  const [committed, setCommitted] = useState<Provider | null>(null);
  const [showRecovery, setShowRecovery] = useState(false);
  const applyPreset = useCallback((advice: PresetAdvice) => {
    dispatch({ type: "preset", value: advice });
    setError(null);
    setModelError(null);
  }, []);
  const presets = useProviderPresets(
    originStoreId === storeId ? storeId : undefined,
    form,
    applyPreset,
  );
  const current = (source: CatalogContext | null) =>
    context.isCurrent(source) && source?.storeId === originStoreId;
  const sourceUnavailable = !originStoreId || originStoreId !== storeId;
  const reloadSaved = async () => {
    const source = context.capture();
    try {
      await onSaved();
    } catch (reason) {
      if (current(source))
        setViewError(`保存已确认，列表暂未刷新：${message(reason)}`);
    }
  };
  const write = useProviderWrite(
    originStoreId,
    (operation) => {
      void catalogSource.cleanup();
      const saved = operation.provider;
      if (!saved) return;
      credentials.confirmBound(saved.credentialRef);
      dispatch({ type: "provider", value: saved });
      setCommitted(saved);
      void reloadSaved();
    },
    storeId ?? null,
  );
  const availableCatalog = catalogCache.catalog;
  const activeCatalog = hasNewKey ? null : availableCatalog;
  const modelOptionsFor = (catalog: typeof availableCatalog) => {
    const options = new Map(
      form.catalog
        .split(/[,，\n]/)
        .map((id) => id.trim())
        .filter(Boolean)
        .map((id) => [id, id]),
    );
    for (const model of catalog?.models ?? [])
      options.set(model.id, model.name);
    if (draft.model)
      options.set(draft.model, options.get(draft.model) ?? draft.model);
    return options;
  };
  const modelOptions = modelOptionsFor(availableCatalog);
  const fetching =
    preparation.kind === "reading" && current(preparation.source);
  const saving =
    write.busy ||
    (preparation.kind === "preparing-save" && current(preparation.source));
  const readSecret = () => {
    const secret = keyInput.current?.value ?? "";
    if (secret && draft.endpoint.includes(secret))
      throw Error("请从服务地址中移除 API Key；本次未保存。");
    return secret;
  };
  const takeSecret = () => {
    const secret = readSecret();
    if (keyInput.current) keyInput.current.value = "";
    setHasNewKey(false);
    return secret;
  };
  const credentialBlocked =
    credentials.ownUnresolved ||
    (credentials.unresolved &&
      (hasNewKey || (!provider.configured && !draft.credentialRef)));
  const canStart = () =>
    !presets.isResolving() &&
    !sourceUnavailable &&
    catalogSource.ready &&
    !committed &&
    !write.unresolved &&
    !write.busy &&
    !credentials.busy &&
    !credentialBlocked &&
    !current(working.current?.source ?? null);
  const readyForKey = async (source: CatalogContext) => {
    const secret = readSecret();
    if (!secret) return true;
    validateCredentialInput(form.label, secret);
    const namespace = await credentials.refreshNamespace();
    if (!current(source)) return false;
    if (namespace.namespaceState !== "active")
      // A new Key uses the current namespace without importing old requests.
      // The backend keeps legacyState, old recovery material and Vault values.
      await credentials.initializeNamespace({
        storeId: namespace.storeId,
        namespaceId: namespace.namespaceId,
        choice: "new",
      });
    return current(source);
  };
  const readModels = async (source: CatalogContext) => {
    let pending = draft;
    if (keyInput.current?.value) {
      const saved = await credentials.create(form.label, takeSecret());
      if (!current(source)) return;
      pending = {
        ...pending,
        credentialRef: saved.credentialRef,
        credentialLabel: saved.credentialLabel,
        modelCatalog: null,
      };
      dispatch({ type: "provider", value: pending });
    }
    await providersApi.writeProtocol(originStoreId);
    if (!current(source)) return;
    const sourceGeneration = catalogSource.capture();
    const route = await catalogSource.prepare(pending);
    if (!current(source) || !catalogSource.current(sourceGeneration)) return;
    const published = await catalogCache.read(
      {
        origin:
          provider.configured || provider.revision > 0
            ? {
                kind: "saved",
                query: {
                  storeId: source.storeId,
                  providerId: provider.id,
                  expectedRevision: String(provider.revision),
                },
                sourceRevision: route?.expectedSourceRevision ?? null,
              }
            : { kind: "new", storeId: source.storeId, providerId: provider.id },
        draft: {
          endpoint: pending.endpoint,
          protocol: pending.protocol,
          vendor: pending.vendor,
          credentialRef: pending.credentialRef,
        },
        source: route?.source ?? { kind: "builtin" },
      },
      catalogKey(pending),
      sourceGeneration,
    );
    if (published && current(source) && catalogSource.current(sourceGeneration))
      dispatch({ type: "manual-model", value: false });
  };
  const savePrepared = async (source: CatalogContext) => {
    // Re-evaluate disclosure after the asynchronous preparation boundary, not
    // from the render that started this save. Registered/user-selected IDs stay.
    const catalog = catalogCache.get();
    const pending = automatic
      ? draft
      : {
          ...draft,
          name:
            draft.name.trim() ||
            (form.preset?.origin.kind === "external"
              ? form.preset.name
              : draft.vendor !== "custom"
                ? providerCatalog[draft.vendor]?.name
                : "") ||
            new URL(draft.endpoint).hostname,
          disabledModels:
            draft.model !== provider.model
              ? (draft.disabledModels ?? []).filter((id) => id !== draft.model)
              : draft.disabledModels,
          credentialLabel: draft.credentialRef ? form.label : null,
          models: [...modelOptionsFor(catalog).keys()],
          modelCatalog: providerCatalogSource(
            {
              ...draft,
              modelCatalog:
                catalogSource.generation > 1 ? null : draft.modelCatalog,
            },
            hasNewKey ? null : catalog,
          ),
        };
    const sourceGeneration = catalogSource.capture();
    await write.start(
      automatic ? "connect" : "save",
      pending,
      async () => {
        if (!keyInput.current?.value) return pending;
        const saved = await credentials.create(form.label, takeSecret());
        const prepared = {
          ...pending,
          credentialRef: saved.credentialRef,
          credentialLabel: saved.credentialLabel,
          modelCatalog: null,
        };
        if (current(source)) dispatch({ type: "provider", value: prepared });
        return prepared;
      },
      async (prepared) => {
        if (!current(source) || !catalogSource.current(sourceGeneration))
          throw Error("目录或后台已变化，本次未提交。");
        return catalogSource.prepare(prepared);
      },
    );
  };
  const run = async (intent: Intent) => {
    if (!canStart()) return;
    const source = context.capture();
    if (!source || !current(source)) return;
    const token = Symbol();
    working.current = { token, source };
    setPreparation({
      kind: intent === "models" ? "reading" : "preparing-save",
      source,
    });
    if (intent === "models") setModelError(null);
    else {
      setError(null);
      setViewError(null);
    }
    try {
      if (
        intent === "models" &&
        !keyInput.current?.value &&
        !draft.credentialRef &&
        !isLoopbackEndpoint(draft.endpoint)
      )
        throw Error("请先填写此供应商的 API Key，再获取模型。");
      await providersApi.writeProtocol(originStoreId);
      if (!current(source) || !(await readyForKey(source))) return;
      if (intent === "models") await readModels(source);
      else {
        setPreparation({ kind: "idle" });
        await savePrepared(source);
      }
    } catch (reason) {
      if (current(source)) {
        if (intent === "models") setModelError(message(reason));
        else setError(message(reason));
      }
    } finally {
      if (working.current?.token === token) {
        working.current = null;
        if (current(source)) setPreparation({ kind: "idle" });
      }
    }
  };
  const adoptCredential = (receipt: RecoverableCredentialReceipt) => {
    const source = context.capture();
    if (
      !current(source) ||
      receipt.storeId !== originStoreId ||
      (receipt.version === 3 &&
        receipt.namespaceId !== credentials.namespace?.namespaceId)
    )
      return;
    if (keyInput.current) keyInput.current.value = "";
    setHasNewKey(false);
    dispatch({
      type: "provider",
      value: {
        ...draft,
        credentialRef: receipt.credentialRef,
        credentialLabel: receipt.credentialLabel,
        modelCatalog:
          receipt.credentialRef === draft.credentialRef
            ? draft.modelCatalog
            : null,
      },
    });
    dispatch({ type: "label", value: receipt.credentialLabel });
    setError(null);
  };
  const busy = saving || fetching || credentials.busy;
  return {
    form,
    dispatch,
    catalogSource,
    presets,
    originStoreId,
    credentials,
    write,
    keyInput,
    hasNewKey,
    activeCatalog,
    modelOptions,
    modelError: modelError ?? catalogCache.error,
    committed,
    viewError,
    showRecovery,
    setShowRecovery,
    shownError:
      error ?? (write.state.kind === "failed" ? write.state.message : null),
    displayModel:
      form.showModel ||
      (write.state.kind === "failed" &&
        ["needs_model", "connection_unknown"].includes(write.state.code ?? "")),
    flags: {
      fetching,
      saving,
      busy,
      sourceUnavailable,
      presetResolving: presets.resolving,
      credentialBlocked,
      fieldsetDisabled:
        busy ||
        presets.resolving ||
        write.unresolved ||
        !!committed ||
        sourceUnavailable,
      submitDisabled:
        !catalogSource.ready ||
        busy ||
        presets.resolving ||
        credentialBlocked ||
        write.unresolved ||
        !!committed ||
        sourceUnavailable ||
        !draft.endpoint.trim() ||
        (!automatic && (!form.vendorChosen || !draft.model.trim())),
      canReadModels:
        catalogSource.ready &&
        !busy &&
        !presets.resolving &&
        !credentialBlocked &&
        !write.unresolved &&
        !!form.vendorChosen &&
        !!draft.endpoint.trim() &&
        !sourceUnavailable &&
        !committed,
    },
    actions: {
      fetchModels: () => run("models"),
      save: () => run("save"),
      takeSecret,
      adoptCredential,
      reloadSaved,
      clearError: () => setError(null),
      keyChanged: (hasValue: boolean) => {
        setHasNewKey(hasValue);
        setError(null);
        setModelError(null);
      },
    },
  };
}
