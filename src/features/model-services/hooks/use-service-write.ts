import { useCallback, useEffect, useRef, useState } from "react";
import { DesktopError } from "@/shared/lib/desktop-command";
import { modelServicesApi } from "../api";
import {
  actionIdentity,
  freezeRequest,
  type ServiceAction,
  type ServiceRequest,
  type ServiceReceipt,
} from "../operation-receipt";
import {
  canStartOperation,
  rememberOperation,
  forgetOperation,
} from "../pending-operations";
type State =
  | { kind: "idle" }
  | { kind: "validating" }
  | { kind: "failed"; message: string }
  | {
      kind: "submitting" | "checking" | "unknown";
      request: ServiceRequest;
      message: string;
    };
const message = (e: unknown) => (e instanceof Error ? e.message : String(e));
const rejected = (e: unknown) =>
  e instanceof DesktopError &&
  !e.outcomeUnknown &&
  [
    "conflict",
    "operation_conflict",
    "operation_store_changed",
    "invalid_input",
    "invalid_provider",
    "invalid_model",
    "invalid_capabilities",
    "unsupported_capabilities",
    "verification_required",
    "capacity",
    "in_use",
    "write_protocol_unavailable",
  ].includes(e.code);
export function useServiceWrite(
  onConfirmed: (receipt: ServiceReceipt) => void,
) {
  const [state, setState] = useState<State>({ kind: "idle" });
  const [progress, setProgress] = useState("");
  const running = useRef(false),
    mounted = useRef(false),
    generation = useRef(0);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      generation.current += 1;
    };
  }, []);
  const current = (token: number) =>
    mounted.current && token === generation.current;
  const accept = (
    receipt: ServiceReceipt,
    request: ServiceRequest,
    token: number,
  ) => {
    if (!current(token)) return;
    try {
      forgetOperation(request);
    } catch {
      /* The receipt still proves commitment; cleanup failure cannot undo it. */
    }
    setState({ kind: "idle" });
    setProgress("");
    window.dispatchEvent(new Event("model-services-changed"));
    try {
      onConfirmed(receipt);
    } catch {
      console.error("local-agent: model-service-confirmed-view");
    }
  };
  const checkRequest = async (request: ServiceRequest, token: number) => {
    if (current(token))
      setState({ kind: "checking", request, message: "正在查询原操作结果…" });
    try {
      accept(await modelServicesApi.operation(request), request, token);
    } catch (e) {
      if (current(token))
        setState({
          kind: "unknown",
          request,
          message: `原操作结果尚未确认；未重新验证或创建新操作。${message(e)}`,
        });
    }
  };
  const submit = async (
    request: ServiceRequest,
    token: number,
    retry: boolean,
  ) => {
    if (current(token))
      setState({
        kind: "submitting",
        request,
        message: retry
          ? "正在提交同一编号的原请求…"
          : "正在提交并保存操作回执…",
      });
    try {
      accept(await modelServicesApi.commit(request), request, token);
    } catch (e) {
      if (!current(token)) return;
      if (!retry && rejected(e)) {
        try {
          forgetOperation(request);
        } catch {
          /* Keep the recoverable ID if storage became unavailable. */
        }
        setState({ kind: "failed", message: message(e) });
      } else await checkRequest(request, token);
    }
  };
  const start = async (
    action: ServiceAction,
    originStoreId: string | undefined,
    verify?: (
      progress: (value: string) => void,
      isCurrent: () => boolean,
      storeId: string,
    ) => Promise<void>,
  ) => {
    if (running.current || state.kind === "unknown") return;
    running.current = true;
    const token = generation.current;
    setState({ kind: "validating" });
    setProgress("正在核对后台结果查询协议…");
    try {
      const storeId =
        await modelServicesApi.requireWriteProtocol(originStoreId);
      if (!originStoreId)
        throw new DesktopError({
          code: "operation_store_changed",
          outcomeUnknown: false,
          message: "无法确认草稿所属后台，请关闭弹层后重新打开并核对。",
        });
      canStartOperation(actionIdentity(action).serviceId, storeId);
      if (!current(token)) return;
      const request = freezeRequest(action, storeId);
      await verify?.(
        (value) => {
          if (current(token)) setProgress(value);
        },
        () => current(token),
        storeId,
      );
      if (!current(token)) return;
      // Persist the exact whitelisted request before a mutation can be sent.
      rememberOperation(request);
      await submit(request, token, false);
    } catch (e) {
      if (current(token)) setState({ kind: "failed", message: message(e) });
    } finally {
      running.current = false;
      if (current(token)) setProgress("");
    }
  };
  const check = async (retry = false) => {
    if (running.current || state.kind !== "unknown") return;
    running.current = true;
    const token = generation.current;
    try {
      if (retry) {
        await modelServicesApi.requireWriteProtocol(state.request.storeId);
        if (current(token)) await submit(state.request, token, true);
      } else await checkRequest(state.request, token);
    } catch (e) {
      if (current(token))
        setState({
          kind: "unknown",
          request: state.request,
          message: message(e),
        });
    } finally {
      running.current = false;
    }
  };
  const reset = useCallback(() => {
    if (!running.current) {
      setState({ kind: "idle" });
      setProgress("");
    }
  }, []);
  return {
    state,
    start,
    check,
    reset,
    progress,
    busy: ["validating", "submitting", "checking"].includes(state.kind),
    uncertain: state.kind === "unknown",
  };
}
