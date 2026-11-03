import { useEffect, useRef, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  backendReadEpoch,
  subscribeBackendRecovery,
} from "@/shared/lib/backend-recovery";
import { providersApi } from "../api";
import {
  LOCAL_TOKEN_CHANGED,
  LOCAL_TOKEN_CANCELLED,
  forgetLocalToken,
  localTokenProtocol,
  localTokenRecovery,
  rememberLocalToken,
  type LocalTokenHint,
  type LocalTokenOperation,
  type LocalTokenResult,
} from "../local-token-operation";
import type { ProviderTransaction } from "../types";

function hintFor(operation: LocalTokenOperation): LocalTokenHint {
  const request = operation.request;
  return {
    version: 1,
    identity: request.identity,
    purpose: request.purpose,
    agentId: request.agentId,
    expectedRevision: request.expectedRevision,
    providerId: request.provider.id,
    providerRevision: request.provider.revision,
    model: request.provider.model,
    ...(request.service
      ? {
          serviceId: request.service.id,
          serviceRevision: request.service.revision,
        }
      : {}),
  };
}
const purposeNames = {
  fixed_route: "配置路由",
  dynamic_binding: "固定动态入口",
  model_service: "模型服务入口",
};

export function LocalTokenRecovery({
  protocol: input,
  agentId,
  onPreview,
}: {
  protocol: unknown;
  agentId?: string;
  onPreview: (preview: ProviderTransaction) => void;
}) {
  const [recovery, setRecovery] = useState(localTokenRecovery);
  const [results, setResults] = useState<Record<string, LocalTokenResult>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [confirmed, setConfirmed] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const generation = useRef(0);
  useEffect(() => {
    const refresh = () => setRecovery(localTokenRecovery());
    const recovered = () => {
      generation.current++;
      setResults({});
      setConfirmed(null);
      setBusy(null);
      setMessage("后台已重新连接，请重新查询原入口结果。");
      refresh();
    };
    window.addEventListener(LOCAL_TOKEN_CHANGED, refresh);
    window.addEventListener("storage", refresh);
    const unsubscribe = subscribeBackendRecovery(recovered);
    const invalidate = () => {
      generation.current++;
    };
    return () => {
      invalidate();
      window.removeEventListener(LOCAL_TOKEN_CHANGED, refresh);
      window.removeEventListener("storage", refresh);
      unsubscribe();
    };
  }, [agentId]);
  let protocol = null;
  try {
    if (input) protocol = localTokenProtocol(input);
  } catch {
    /* Keep original hints while the source is unavailable. */
  }
  const hints = recovery.hints.filter(
    (hint) => !agentId || hint.agentId === agentId,
  );
  const discovered =
    protocol?.pending.filter(
      (operation) =>
        (!agentId || operation.request.agentId === agentId) &&
        !recovery.hints.some(
          (hint) =>
            hint.identity.operationId === operation.ownerId &&
            hint.identity.storeId === operation.request.identity.storeId &&
            hint.identity.namespaceId ===
              operation.request.identity.namespaceId,
        ),
    ) ?? [];
  if (!hints.length && !discovered.length && !recovery.invalid && !message)
    return null;
  const current = (hint: LocalTokenHint) =>
    protocol?.namespaceState === "active" &&
    protocol.storeId === hint.identity.storeId &&
    protocol.namespaceId === hint.identity.namespaceId;
  const run = async (
    hint: LocalTokenHint,
    action: "query" | "continue" | "cancel",
  ) => {
    if (
      busy ||
      !current(hint) ||
      (action !== "query" && recovery.invalid) ||
      (action === "continue" &&
        (confirmed !== hint.identity.operationId ||
          !results[hint.identity.operationId]?.canContinue))
    )
      return;
    const epoch = backendReadEpoch(),
      ticket = ++generation.current,
      id = hint.identity.operationId;
    setBusy(id);
    setConfirmed(null);
    setMessage(null);
    try {
      const result =
        action === "query"
          ? await providersApi.queryLocalToken(hint.identity, hint)
          : action === "continue"
            ? await providersApi.continueLocalToken(hint.identity, hint)
            : await providersApi.cancelLocalToken(hint.identity, hint);
      if (ticket !== generation.current || epoch !== backendReadEpoch()) return;
      setResults((previous) => ({ ...previous, [id]: result }));
      if (result.cancellation) {
        forgetLocalToken(hint.identity);
        window.dispatchEvent(
          new CustomEvent(LOCAL_TOKEN_CANCELLED, { detail: hint.identity }),
        );
        setMessage(
          "原编号已阻止后续创建，已有本机入口材料继续保留；历史配置如有变化需另行核查。",
        );
      } else if (action === "continue" && result.preview) {
        forgetLocalToken(hint.identity);
        onPreview(result.preview);
        setMessage("原入口预览已找回，请核对后按原流程应用。");
      } else if (
        action === "cancel" &&
        (result.operation?.status === "cancelled" ||
          result.preview?.state === "cancelled")
      ) {
        forgetLocalToken(hint.identity);
        window.dispatchEvent(
          new CustomEvent(LOCAL_TOKEN_CANCELLED, { detail: hint.identity }),
        );
        setMessage(
          "已停止这次创建或待应用预览；已写入的凭据、路由和绑定继续保留。",
        );
      } else if (action === "cancel") {
        setMessage(
          "原预览已存在，当前状态为“" +
            (result.preview?.state ?? "待核查") +
            "”；已应用配置不会自动撤回，请采用预览后按原流程处理。",
        );
      } else setMessage(result.sourceMessage);
      window.dispatchEvent(new Event("provider-status-changed"));
      window.dispatchEvent(new Event("model-services-changed"));
    } catch (error) {
      if (ticket === generation.current && epoch === backendReadEpoch())
        setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      if (ticket === generation.current && epoch === backendReadEpoch())
        setBusy(null);
    }
  };
  return (
    <section className="space-y-3" aria-label="本机入口结果核查">
      {recovery.invalid && (
        <Alert status="warning">
          <AlertTitle>暂存记录需要核查</AlertTitle>
          <AlertDescription>
            原记录不可读取或容量已满，原字节已保留；新的入口暂不可提交。
          </AlertDescription>
        </Alert>
      )}
      {message && (
        <p role="status" className="text-body text-fg-muted">
          {message}
        </p>
      )}
      {discovered.map((operation) => (
        <Alert status="warning" key={operation.ownerId}>
          <AlertTitle>
            后台有待核查的{purposeNames[operation.request.purpose]}
          </AlertTitle>
          <AlertDescription>
            <p className="break-all">
              {operation.request.agentId} · {operation.ownerId}
            </p>
            <p>
              {operation.request.provider.name} ·{" "}
              {operation.request.provider.model}
            </p>
            <Button
              size="sm"
              variant="tertiary"
              disabled={
                !!busy ||
                recovery.invalid ||
                protocol?.namespaceState !== "active"
              }
              onClick={() => {
                try {
                  const hint = hintFor(operation);
                  rememberLocalToken(hint);
                  void run(hint, "query");
                } catch (error) {
                  setMessage(
                    error instanceof Error ? error.message : String(error),
                  );
                }
              }}
            >
              保留原编号并查询
            </Button>
          </AlertDescription>
        </Alert>
      ))}
      {hints.map((hint) => {
        const id = hint.identity.operationId,
          result = results[id],
          source = result?.operation?.request;
        return (
          <Alert
            status="warning"
            key={hint.identity.storeId + hint.identity.namespaceId + id}
          >
            <AlertTitle>待核查的{purposeNames[hint.purpose]}</AlertTitle>
            <AlertDescription>
              <div className="space-y-2">
                <p className="break-all">
                  {hint.agentId} · 原编号：{id}
                </p>
                {!current(hint) && (
                  <p>请连接原后台并核查原凭据恢复空间，再查询这次入口。</p>
                )}
                {source && (
                  <>
                    <p>
                      {source.provider.name} · {source.provider.model}
                    </p>
                    <p className="break-all">
                      配置：{source.configurationPath}
                    </p>
                    <p className="break-all">
                      端点：{source.provider.endpoint}
                    </p>
                  </>
                )}
                <div className="flex flex-wrap gap-2">
                  <Button
                    size="sm"
                    variant="tertiary"
                    disabled={!!busy || !current(hint)}
                    onClick={() => void run(hint, "query")}
                  >
                    查询原入口结果
                  </Button>
                  <Button
                    size="sm"
                    variant="tertiary"
                    disabled={!!busy || !current(hint) || recovery.invalid}
                    onClick={() => void run(hint, "cancel")}
                  >
                    停止这次创建或预览
                  </Button>
                </div>
                {result?.canContinue && (
                  <>
                    <label className="flex items-center gap-2">
                      <Checkbox
                        aria-label="已核对原入口来源"
                        checked={confirmed === id}
                        disabled={!!busy || !current(hint) || recovery.invalid}
                        onCheckedChange={(value) =>
                          setConfirmed(value === true ? id : null)
                        }
                      />
                      <span>我已核对原配置、供应商与模型，继续原入口</span>
                    </label>
                    <Button
                      size="sm"
                      variant="neutral"
                      disabled={
                        !!busy ||
                        !current(hint) ||
                        recovery.invalid ||
                        confirmed !== id
                      }
                      onClick={() => void run(hint, "continue")}
                    >
                      继续原入口
                    </Button>
                  </>
                )}
                {result?.preview && (
                  <Button
                    size="sm"
                    variant="neutral"
                    disabled={!!busy || !current(hint) || recovery.invalid}
                    onClick={() => {
                      try {
                        forgetLocalToken(hint.identity);
                        onPreview(result.preview!);
                      } catch (error) {
                        setMessage(
                          error instanceof Error
                            ? error.message
                            : String(error),
                        );
                      }
                    }}
                  >
                    采用已核查预览
                  </Button>
                )}
              </div>
            </AlertDescription>
          </Alert>
        );
      })}
    </section>
  );
}
