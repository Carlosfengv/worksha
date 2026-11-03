import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { DesktopError } from "@/shared/lib/desktop-command";
import { settingsApi } from "./api";
import {
  freezeGatewayPending,
  gatewayCompensation,
  sameGatewayRequest,
} from "./gateway-operation";
import {
  readGatewayRecovery,
  rememberGatewayOperation,
  requireGatewayRecovery,
  forgetGatewayOperation,
  GATEWAY_OPERATIONS_CHANGED,
} from "./pending-gateway";
import type {
  PendingGatewayOperation,
  GatewayOperationOutcome,
  GatewayClientDraft,
} from "./types";
const message = (e: unknown) => (e instanceof Error ? e.message : String(e));
export function useGatewayOperations(
  storeId: string | undefined,
  refresh: () => Promise<void>,
) {
  const [pending, setPending] = useState(readGatewayRecovery);
  const [busy, setBusy] = useState(false),
    [error, setError] = useState<string | null>(null),
    [notice, setNotice] = useState<string | null>(null);
  const [confirmed, setConfirmed] = useState<
    Record<string, GatewayOperationOutcome>
  >({});
  const [settled, setSettled] = useState<
    Record<string, { pending: PendingGatewayOperation; label: string }>
  >({});
  const mounted = useRef(false),
    running = useRef(false),
    current = useRef(storeId);
  useLayoutEffect(() => {
    current.current = storeId;
  }, [storeId]);
  useEffect(() => {
    mounted.current = true;
    const reload = () => {
      const state = readGatewayRecovery();
      setPending(state);
      if (state.available) {
        const ids = new Set(
          state.requests.map((row) => row.request.operationId),
        );
        setConfirmed((old) =>
          Object.fromEntries(Object.entries(old).filter(([id]) => ids.has(id))),
        );
        setSettled((old) =>
          Object.fromEntries(Object.entries(old).filter(([id]) => ids.has(id))),
        );
      }
    };
    window.addEventListener("storage", reload);
    window.addEventListener(GATEWAY_OPERATIONS_CHANGED, reload);
    return () => {
      mounted.current = false;
      window.removeEventListener("storage", reload);
      window.removeEventListener(GATEWAY_OPERATIONS_CHANGED, reload);
    };
  }, []);
  async function perform<T>(work: () => Promise<T>): Promise<T | undefined> {
    if (running.current) return;
    running.current = true;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      return await work();
    } catch (reason) {
      if (mounted.current) setError(message(reason));
      return undefined;
    } finally {
      running.current = false;
      if (mounted.current) setBusy(false);
    }
  }
  function finish(row: PendingGatewayOperation, text: string) {
    if (!mounted.current) return;
    let warning = "";
    try {
      forgetGatewayOperation(row);
      setSettled((old) => {
        const next = { ...old };
        delete next[row.request.operationId];
        return next;
      });
      setConfirmed((old) => {
        const next = { ...old };
        delete next[row.request.operationId];
        return next;
      });
    } catch (e) {
      warning = message(e);
      setSettled((old) => ({
        ...old,
        [row.request.operationId]: {
          pending: freezeGatewayPending(row),
          label: text,
        },
      }));
    }
    setNotice(text + (warning ? ` ${warning}` : ""));
    if (current.current === row.request.storeId) void refresh();
  }
  function keepIssued(
    row: PendingGatewayOperation,
    out: GatewayOperationOutcome,
  ) {
    if (!mounted.current) return;
    setConfirmed((old) => ({ ...old, [row.request.operationId]: out }));
    setNotice(
      "原后台签发已确认，连接文件状态仍待核对；没有重新签发密钥。请核对原文件或明确撤销原客户端。",
    );
  }
  async function send(row: PendingGatewayOperation, fresh = false) {
    requireGatewayRecovery(row);
    if (current.current !== row.request.storeId)
      throw Error("请连接并确认原后台后重试，未改用当前来源或新编号。");
    if (row.request.action.kind === "revoke") {
      const result = await settingsApi.commitGatewayClientOperation(
        row.request,
      );
      finish(
        row,
        result.status === "committed"
          ? "原后台客户端已确认撤销。其他客户端和已有请求继续运行。"
          : "原后台客户端当前已撤销；未将当前状态伪装为原编号回执。其他客户端和已有请求继续运行。",
      );
      return false;
    }
    const result = await settingsApi.saveGatewayClientOperation(row);
    if (!mounted.current) return false;
    if (result.status === "cancelled") {
      if (fresh) finish(row, "已取消本次选择，未创建客户端。");
      else
        setNotice(
          "已取消本次保存选择；原客户端操作仍待核查，保留原编号与补偿编号。",
        );
    } else if (result.status === "notIssued") {
      if (fresh) finish(row, `本次未新增客户端。${result.reason}`);
      else
        setNotice(`本次尝试未新增客户端；原操作仍保留待核查。${result.reason}`);
    } else if (result.status === "saved") {
      finish(row, `原后台客户端已创建，连接文件已保存为 ${result.fileName}。`);
      return true;
    } else if (result.status === "revoked") {
      finish(row, result.reason);
    } else keepIssued(row, result.outcome);
    return false;
  }
  const startCreate = (draft: GatewayClientDraft, originalStoreId: string) =>
    perform(async () => {
      if (current.current !== originalStoreId)
        throw Error("客户端草稿不属于当前已确认后台，未发送签发。");
      const row = freezeGatewayPending({
        request: {
          storeId: originalStoreId,
          operationId: crypto.randomUUID(),
          action: { kind: "create", draft },
        },
        compensationOperationId: crypto.randomUUID(),
      });
      rememberGatewayOperation(row);
      return send(row, true);
    });
  const startRevoke = (clientId: string, originalStoreId: string) =>
    perform(async () => {
      if (current.current !== originalStoreId)
        throw Error("客户端来源已变化，未向新后台撤销。");
      const row = freezeGatewayPending({
        request: {
          storeId: originalStoreId,
          operationId: crypto.randomUUID(),
          action: { kind: "revoke", clientId },
        },
      });
      rememberGatewayOperation(row);
      return send(row, true);
    });
  const retry = (row: PendingGatewayOperation) => perform(() => send(row));
  const check = (row: PendingGatewayOperation) =>
    perform(async () => {
      requireGatewayRecovery(row);
      const out = await settingsApi.gatewayClientOperation(row.request);
      if (row.request.action.kind === "revoke") {
        finish(row, "原后台客户端撤销已确认。");
        return;
      }
      keepIssued(row, out);
      try {
        await settingsApi.gatewayClientOperation(gatewayCompensation(row));
        finish(
          row,
          "原签发及原客户端撤销均已确认，请核对目标文件；没有重新签发密钥。",
        );
      } catch (e) {
        if (
          !(
            e instanceof DesktopError &&
            e.code === "operation_not_found" &&
            !e.outcomeUnknown
          ) &&
          mounted.current
        )
          setError(`签发已确认，原撤销状态尚未核对。${message(e)}`);
      }
    });
  const revokeIssued = (row: PendingGatewayOperation) =>
    perform(async () => {
      requireGatewayRecovery(row);
      const found = confirmed[row.request.operationId];
      if (!found || !sameGatewayRequest(found.request, row.request))
        throw Error("请先核查原签发结果，再明确撤销。");
      if (current.current !== row.request.storeId)
        throw Error("请连接原后台后撤销，未改用当前来源。");
      const result = await settingsApi.commitGatewayClientOperation(
        gatewayCompensation(row),
      );
      finish(
        row,
        result.status === "committed"
          ? "原后台本次签发的客户端撤销回执已确认，请核对目标文件。"
          : "原后台本次客户端当前已撤销，原补偿编号回执未确认，请核对目标文件。",
      );
    });
  const acknowledge = (row: PendingGatewayOperation) =>
    perform(async () => {
      requireGatewayRecovery(row);
      const found = confirmed[row.request.operationId];
      if (!found || !sameGatewayRequest(found.request, row.request))
        throw Error("原签发结果尚未核对，恢复编号已保留。");
      finish(
        row,
        "已按你的文件核对结果移除本机恢复记录；原客户端授权保持原状态。",
      );
    });
  const removeKnown = (row: PendingGatewayOperation) =>
    perform(async () => {
      requireGatewayRecovery(row);
      const known = settled[row.request.operationId];
      if (
        !known ||
        JSON.stringify(freezeGatewayPending(known.pending)) !==
          JSON.stringify(freezeGatewayPending(row))
      )
        throw Error("原确认结果与保留记录不一致，未移除编号。");
      finish(row, known.label);
    });
  return {
    pending,
    busy,
    error,
    notice,
    confirmed,
    settled,
    removeKnown,
    startCreate,
    startRevoke,
    retry,
    check,
    revokeIssued,
    acknowledge,
  };
}
