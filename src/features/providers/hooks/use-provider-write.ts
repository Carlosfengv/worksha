import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { subscribeBackendRecovery } from "@/shared/lib/backend-recovery";
import { DesktopError } from "@/shared/lib/desktop-command";
import { providerWriteRejected } from "../error-contract";
import {
  attachDraftGrantOperation,
  finishDraftGrantOperation,
  hasDraftGrantOperation,
  reserveDraftGrantOperation,
  sweepClosedDraftGrants,
} from "../draft-grants";
import {
  providersApi,
  providerCatalogApi,
  providerSourceOperationApi,
} from "../api";
import {
  selectedProviderRoute,
  isExplicitProviderSource,
  type ProviderWriteRoute,
} from "../catalog-operation";
import type { Provider } from "../types";
import {
  freezeProviderRequest,
  type ProviderOperation,
  type ProviderWriteRequest,
} from "../operation-receipt";
import {
  canStartProviderOperation,
  rememberProviderOperation,
  forgetProviderOperation,
  providerOperationCatalog,
} from "../pending-operations";
type State =
  | { kind: "idle" | "cancelled" }
  | { kind: "preparing" }
  | {
      kind: "submitting" | "checking" | "cancelling" | "unknown";
      request: ProviderWriteRequest;
      catalog: ProviderWriteRoute | null;
      message: string;
    }
  | { kind: "failed"; message: string; code?: string };
const message = (reason: unknown) =>
  reason instanceof Error ? reason.message : String(reason);
export function useProviderWrite(
  originStoreId: string | undefined,
  onResolved: (operation: ProviderOperation) => void,
  activeStoreId: string | null | undefined = originStoreId,
) {
  const [state, setState] = useState<State>({ kind: "idle" });
  const mounted = useRef(false),
    generation = useRef(0),
    running = useRef<number | null>(null);
  const onResolvedRef = useRef(onResolved);
  useEffect(() => {
    if (activeStoreId)
      void sweepClosedDraftGrants(activeStoreId).catch(() => null);
  }, [activeStoreId]);
  useLayoutEffect(() => {
    onResolvedRef.current = onResolved;
  }, [onResolved]);
  useLayoutEffect(() => {
    mounted.current = true;
    generation.current++;
    running.current = null;
    setState({ kind: "idle" });
    return () => {
      mounted.current = false;
      running.current = null;
    };
  }, [activeStoreId]);
  useEffect(
    () =>
      subscribeBackendRecovery(() => {
        // Invalidate this view's publication token. Keep the durable request;
        // reconnect never replays, cancels or reverses a business action.
        generation.current += 1;
        running.current = null;
        if (activeStoreId)
          void sweepClosedDraftGrants(activeStoreId).catch(() => null);
        setState((previous) =>
          "request" in previous
            ? {
                kind: "unknown",
                request: previous.request,
                catalog: previous.catalog,
                message:
                  "后台连接已恢复，原请求仍保留；请核查原结果，未重新提交。",
              }
            : { kind: "idle" },
        );
      }),
    [activeStoreId],
  );
  const current = useCallback(
    (token: number) => mounted.current && token === generation.current,
    [],
  );
  const accept = useCallback(
    (
      operation: ProviderOperation,
      token: number,
      catalog: ProviderWriteRoute | null,
    ) => {
      if (!current(token)) return;
      if (operation.status === "pending") {
        setState({
          kind: "unknown",
          request: operation.request,
          catalog,
          message: "原连接仍待核查；查询不会重新探测，可再次查询或取消原操作。",
        });
        return;
      }
      void finishDraftGrantOperation(operation, catalog).catch(() => null);
      try {
        forgetProviderOperation(operation.request, catalog);
      } catch {
        /* Commitment remains valid; the recovery list can query again. */
      }
      if (operation.status === "failed") {
        setState({
          kind: "failed",
          code: operation.error?.code,
          message: operation.error?.message ?? "连接未保存，请核查后重试。",
        });
      } else {
        setState({
          kind: operation.status === "cancelled" ? "cancelled" : "idle",
        });
        if (operation.status === "committed") {
          window.dispatchEvent(new Event("providers-changed"));
        }
      }
      try {
        onResolvedRef.current(operation);
      } catch {
        console.error("local-agent: provider-resolved-view");
      }
    },
    [current],
  );
  const checkRequest = useCallback(
    async (
      request: ProviderWriteRequest,
      token: number,
      catalog: ProviderWriteRoute | null,
    ) => {
      if (current(token))
        setState({
          kind: "checking",
          request,
          catalog,
          message: "正在查询原操作结果…",
        });
      try {
        accept(
          await (catalog && isExplicitProviderSource(catalog)
            ? providerSourceOperationApi.perform("query", request, catalog)
            : catalog
              ? providerCatalogApi.operation({ operation: request, catalog })
              : providersApi.operation(request)),
          token,
          catalog,
        );
      } catch (reason) {
        if (current(token))
          setState({
            kind: "unknown",
            request,
            catalog,
            message: `原结果尚未确认，已保留原请求；没有重新探测。${message(reason)}`,
          });
      }
    },
    [accept, current],
  );
  const submit = useCallback(
    async (
      request: ProviderWriteRequest,
      token: number,
      retry: boolean,
      catalog: ProviderWriteRoute | null,
    ) => {
      if (current(token))
        setState({
          kind: "submitting",
          request,
          catalog,
          message:
            request.kind === "connect"
              ? "正在识别并验证…"
              : "正在保存并记录回执…",
        });
      let ownsDraftGrant = false;
      try {
        ownsDraftGrant = hasDraftGrantOperation(request, catalog);
        await attachDraftGrantOperation(request, catalog);
        if (!current(token)) return;
        accept(
          await (catalog && isExplicitProviderSource(catalog)
            ? providerSourceOperationApi.perform("commit", request, catalog)
            : catalog
              ? providerCatalogApi.commit({ operation: request, catalog })
              : providersApi.commitOperation(request)),
          token,
          catalog,
        );
      } catch (reason) {
        if (!current(token)) return;
        // Attach can already have pinned authority before business admission
        // rejects. Preserve that original request so cancellation can confirm
        // a terminal result and release the exact grant from any recovery view.
        if (!retry && !ownsDraftGrant && providerWriteRejected(reason)) {
          try {
            forgetProviderOperation(request, catalog);
          } catch {
            /* Keep recovery metadata if removal failed. */
          }
          setState({
            kind: "failed",
            message: message(reason),
            code: reason instanceof DesktopError ? reason.code : undefined,
          });
        } else await checkRequest(request, token, catalog);
      }
    },
    [accept, checkRequest, current],
  );
  const start = useCallback(
    async (
      kind: ProviderWriteRequest["kind"],
      draft: Provider,
      prepare?: () => Promise<Provider>,
      selectCatalog?: (
        provider: Provider,
      ) => Promise<ProviderWriteRoute | null>,
    ) => {
      if (running.current !== null) return;
      const token = ++generation.current;
      running.current = token;
      setState({ kind: "preparing" });
      try {
        if (!originStoreId || originStoreId !== activeStoreId)
          throw new DesktopError({
            code: "operation_store_changed",
            outcomeUnknown: false,
            message: "草稿属于原来的本机数据库，请连接原后台后核查。",
          });
        await providersApi.writeProtocol(originStoreId);
        if (!current(token)) return;
        canStartProviderOperation(originStoreId!, draft.id);
        const provider = prepare ? await prepare() : draft;
        if (!current(token)) return;
        const rawCatalog = selectCatalog ? await selectCatalog(provider) : null;
        if (!current(token)) return;
        const protocol = await providersApi.writeProtocol(originStoreId);
        if (!current(token)) return;
        const request = freezeProviderRequest(protocol, kind, provider);
        const catalog =
          rawCatalog === null
            ? null
            : selectedProviderRoute(rawCatalog, request);
        if (rawCatalog !== null && !catalog)
          throw Error("目录选择无法核查，本次未提交，也未改用内置目录。");
        // Ownership correlation must be durable before the original hint. A
        // local quota error here must not create an uncorrelated unknown write.
        reserveDraftGrantOperation(request, catalog);
        rememberProviderOperation(request, catalog);
        await submit(request, token, false, catalog);
      } catch (reason) {
        if (current(token))
          setState({
            kind: "failed",
            message: message(reason),
            code: reason instanceof DesktopError ? reason.code : undefined,
          });
      } finally {
        if (running.current === token) running.current = null;
      }
    },
    [activeStoreId, current, originStoreId, submit],
  );
  const resume = useCallback(
    async (
      action: "check" | "retry" | "cancel",
      frozen: ProviderWriteRequest,
      selectedCatalog?: ProviderWriteRoute | null,
    ) => {
      if (running.current !== null) return;
      const token = ++generation.current;
      running.current = token;
      let catalog = selectedCatalog ?? null;
      try {
        catalog = providerOperationCatalog(frozen, selectedCatalog);
        if (
          !originStoreId ||
          originStoreId !== activeStoreId ||
          frozen.storeId !== originStoreId
        ) {
          if (current(token))
            setState({
              kind: "unknown",
              request: frozen,
              catalog,
              message: "此操作属于另一个本机数据库，请连接原后台后核查。",
            });
          return;
        }
        if (action === "check") await checkRequest(frozen, token, catalog);
        else if (action === "retry") await submit(frozen, token, true, catalog);
        else {
          if (current(token))
            setState({
              kind: "cancelling",
              request: frozen,
              catalog,
              message: "正在停止原操作的后续保存…",
            });
          try {
            const operation = await providersApi.cancelOperation(frozen);
            // Terminal choice bookkeeping is independent of plugin availability.
            if (catalog)
              await (
                isExplicitProviderSource(catalog)
                  ? providerSourceOperationApi.perform("query", frozen, catalog)
                  : providerCatalogApi.operation({ operation: frozen, catalog })
              ).catch(() => null);
            accept(operation, token, catalog);
          } catch (reason) {
            if (current(token))
              setState({
                kind: "unknown",
                request: frozen,
                catalog,
                message: `尚未确认取消，请保留原请求。${message(reason)}`,
              });
          }
        }
      } catch (reason) {
        if (current(token))
          setState({
            kind: "unknown",
            request: frozen,
            catalog,
            message: message(reason),
          });
      } finally {
        if (running.current === token) running.current = null;
      }
    },
    [accept, activeStoreId, checkRequest, current, originStoreId, submit],
  );
  return {
    state,
    start,
    resume,
    busy: ["preparing", "submitting", "checking", "cancelling"].includes(
      state.kind,
    ),
    unresolved: state.kind === "unknown",
  };
}
