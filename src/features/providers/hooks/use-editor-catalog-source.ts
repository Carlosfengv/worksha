import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type {
  Policy,
  Target,
} from "../../../../contracts/domain/provider-access/types";
import type { SourceState } from "../../../../contracts/domain/provider-source/types";
import type { Choice } from "../../../../contracts/domain/provider-source/types";
import {
  providerCatalogApi,
  providerSourceApi,
  scopedProviderCatalogApi,
} from "../api";
import type { ExplicitProviderSource } from "../catalog-operation";
import {
  acquireEditorDraftGrant,
  closeEditorDraftGrants,
} from "../draft-grants";
import { message } from "../display";
import type { Provider } from "../types";
import { useCatalogContext } from "./use-catalog-context";

// A remembered choice is not authority. Draft grants never become Saved grants.
export function useEditorCatalogSource(provider: Provider, storeId?: string) {
  const {
    capture: captureContext,
    isCurrent: contextCurrent,
    epoch,
  } = useCatalogContext(storeId);
  const [state, setState] = useState<SourceState | null>(null);
  const [loading, setLoading] = useState(
    provider.configured || provider.revision > 0,
  );
  const [recordError, setRecordError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [plugins, setPlugins] = useState<
    Awaited<ReturnType<typeof providerCatalogApi.adapters>>
  >([]);
  const [choice, setChoice] = useState<Choice | null>(null);
  const [policy, setPolicy] = useState<Policy | null>(null);
  const [consent, setConsent] = useState(false);
  const [privateNetwork, setPrivateNetwork] = useState(false);
  const [generation, setGeneration] = useState(0);
  const generationRef = useRef(0);
  const [editorId] = useState(() => crypto.randomUUID());
  const bump = () => {
    generationRef.current++;
    setGeneration(generationRef.current);
  };
  const capture = () => generationRef.current;
  const current = (g: number) => g === generationRef.current;
  const [reload, setReload] = useState(0);
  useLayoutEffect(() => {
    generationRef.current++;
    setGeneration(generationRef.current);
    setState(null);
    setChoice(null);
    setPolicy(null);
    setConsent(false);
    setError(null);
    setRecordError(null);
    setLoading(provider.configured || provider.revision > 0);
  }, [
    storeId,
    epoch,
    provider.id,
    provider.revision,
    provider.configured,
    reload,
  ]);
  useEffect(() => {
    if (!provider.configured && provider.revision === 0) return;
    const source = captureContext(),
      g = generationRef.current;
    if (!source) return;
    let cancelled = false;
    void providerSourceApi
      .state({
        storeId: source.storeId,
        providerId: provider.id,
        expectedRevision: String(provider.revision),
      })
      .then((value) => {
        if (cancelled || !contextCurrent(source) || !current(g)) return;
        setState(value);
        setChoice(value.choice ?? null);
        setPolicy(value.policy ?? null);
        if (value.kind !== "builtin") setExpanded(true);
        setLoading(false);
      })
      .catch((reason: unknown) => {
        if (!cancelled && contextCurrent(source) && current(g)) {
          setRecordError(message(reason));
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [
    captureContext,
    contextCurrent,
    epoch,
    provider.id,
    provider.revision,
    provider.configured,
    reload,
  ]);
  useEffect(() => {
    if (!expanded) return;
    const source = captureContext();
    if (!source) return;
    let cancelled = false;
    void providerCatalogApi
      .adapters()
      .then((items) => {
        if (!cancelled && contextCurrent(source)) setPlugins(items);
      })
      .catch((reason: unknown) => {
        if (!cancelled && contextCurrent(source)) setError(message(reason));
      });
    return () => {
      cancelled = true;
    };
  }, [expanded, captureContext, contextCurrent, epoch, reload]);
  const cleanup = async () => {
    if (storeId)
      await closeEditorDraftGrants(editorId, storeId).catch(() => null);
  };
  const cleanupRef = useRef(cleanup);
  useLayoutEffect(() => {
    cleanupRef.current = cleanup;
  });
  useEffect(
    () => () => {
      void cleanupRef.current();
    },
    [],
  );
  const choose = async (next: Choice | null) => {
    bump();
    const g = generationRef.current;
    setChoice(next);
    setPolicy(null);
    setConsent(false);
    setPrivateNetwork(false);
    setError(null);
    if (!next) return;
    const source = captureContext();
    if (!source) return;
    try {
      const result = await providerCatalogApi.state(next);
      if (!contextCurrent(source) || !current(g)) return;
      setPolicy(result.policy ?? null);
      if (!result.canBind || !result.policy)
        setError("所选目录暂不可授权，请检查插件状态与权限。");
    } catch (reason) {
      if (contextCurrent(source) && current(g)) setError(message(reason));
    }
  };
  const prepare = async (
    draft: Provider,
  ): Promise<ExplicitProviderSource | null> => {
    const source = captureContext(),
      g = capture();
    if (!source || loading || recordError || error)
      throw Error(recordError ?? error ?? "目录来源尚未核对，请稍后重试。");
    if ((provider.configured || provider.revision > 0) && !state)
      throw Error("原目录来源尚不可核查，本次未提交。");
    if (state?.kind === "pending")
      throw Error("原目录操作仍待核查，请先处理恢复列表。");
    const expectedSourceRevision = state?.sourceRevision ?? null;
    // Unchanged new-native onboarding retains its original direct-submit path.
    if (!choice)
      return state || expectedSourceRevision
        ? { source: { kind: "builtin" }, expectedSourceRevision }
        : null;
    if (!consent || !policy) throw Error("请核对并同意本次外部目录授权。");
    if (draft.protocol !== policy.baseProtocol)
      throw Error(
        `所选目录要求 ${policy.baseProtocol} 协议，请在高级设置中明确选择；地址没有自动改写。`,
      );
    if (!draft.vendor) throw Error("请先明确供应商范围，再授权目录。");
    const target = {
      kind: "draft",
      label: Array.from(draft.name.trim() || "模型连接")
        .slice(0, 100)
        .join(""),
      endpoint: draft.endpoint,
      protocol: draft.protocol,
      vendor: draft.vendor,
      credentialRef: draft.credentialRef,
    } satisfies Target;
    if (!contextCurrent(source) || !current(g))
      throw Error("目录或后台已变化，本次未提交。");
    const catalog = await acquireEditorDraftGrant(
      editorId,
      provider.id,
      source.storeId,
      {
        catalog: { pluginId: choice.pluginId, digest: choice.digest },
        draft: {
          endpoint: target.endpoint,
          protocol: target.protocol,
          vendor: target.vendor,
          credentialRef: target.credentialRef ?? null,
        },
        label: target.label,
        allowPrivateNetwork: privateNetwork,
      },
      g,
    );
    if (!contextCurrent(source) || !current(g)) {
      void cleanup();
      throw Error("目录或后台已变化，本次未提交。");
    }
    return { source: { kind: "external", catalog }, expectedSourceRevision };
  };
  const authorizeSaved = async () => {
    const source = captureContext(),
      g = capture();
    if (!source || !state || !choice || !consent || !policy || loading) return;
    setLoading(true);
    setError(null);
    try {
      const query = {
        storeId: source.storeId,
        providerId: provider.id,
        expectedRevision: String(provider.revision),
      };
      const target: Target = {
        kind: "saved",
        providerId: provider.id,
        expectedRevision: String(provider.revision),
      };
      const result = await scopedProviderCatalogApi.change({
        storeId: source.storeId,
        request: {
          ...choice,
          action: { kind: "bind", target, allowPrivateNetwork: privateNetwork },
        },
      });
      if (!contextCurrent(source) || !current(g)) return;
      const binding = result.bindings.find(
        (b) =>
          b.authorized &&
          b.providerId === provider.id &&
          b.expectedRevision === String(provider.revision),
      );
      if (!binding) throw Error("已保存连接授权尚未确认。");
      const selected = await providerSourceApi.select(
        query,
        state.sourceRevision ?? null,
        { ...choice, bindingId: binding.bindingId, target },
      );
      if (contextCurrent(source) && current(g)) {
        bump();
        setState(selected);
        setConsent(false);
      }
    } catch (reason) {
      if (contextCurrent(source) && current(g)) setError(message(reason));
    } finally {
      if (contextCurrent(source)) setLoading(false);
    }
  };
  return {
    state,
    loading,
    error: recordError ?? error,
    ready:
      !loading &&
      !recordError &&
      !error &&
      state?.kind !== "pending" &&
      (!choice || (!!policy && consent)),
    expanded,
    plugins,
    choice,
    policy,
    consent,
    privateNetwork,
    generation,
    capture,
    current,
    prepare,
    cleanup,
    authorizeSaved,
    open: () => setExpanded(true),
    choose,
    retry: () => {
      bump();
      setReload((n) => n + 1);
    },
    setConsent: (value: boolean) => {
      bump();
      setConsent(value);
    },
    setPrivateNetwork: (value: boolean) => {
      bump();
      setPrivateNetwork(value);
      setConsent(false);
    },
  };
}
