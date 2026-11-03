import { useEffect, useRef, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { modelServicesApi } from "../api";
import {
  pendingOperations,
  forgetOperation,
  OPERATIONS_CHANGED,
} from "../pending-operations";
import type { ServiceRequest } from "../operation-receipt";
function read() {
  try {
    return pendingOperations();
  } catch {
    return { requests: [], invalid: 1 };
  }
}
export function ModelServiceOperationRecovery() {
  const [pending, setPending] = useState(read),
    [busy, setBusy] = useState<string | null>(null),
    [notice, setNotice] = useState("");
  const mounted = useRef(false),
    running = useRef(false);
  useEffect(() => {
    mounted.current = true;
    const reload = () => setPending(read());
    window.addEventListener(OPERATIONS_CHANGED, reload);
    window.addEventListener("storage", reload);
    return () => {
      mounted.current = false;
      window.removeEventListener(OPERATIONS_CHANGED, reload);
      window.removeEventListener("storage", reload);
    };
  }, []);
  const check = async (request: ServiceRequest, retry: boolean) => {
    if (running.current) return;
    running.current = true;
    setBusy(request.operationId);
    setNotice("");
    try {
      if (retry) await modelServicesApi.requireWriteProtocol(request.storeId);
      if (!mounted.current) return;
      const receipt = retry
        ? await modelServicesApi.commit(request)
        : await modelServicesApi.operation(request);
      if (!mounted.current) return;
      try {
        forgetOperation(request);
      } catch {
        /* Confirmation is independent of recovery metadata cleanup. */
      }
      setNotice(
        `原${receipt.action === "save" ? "发布" : "删除"}已提交（版本 ${receipt.committedRevision}）；当前配置请查看重新读取的目录。`,
      );
      window.dispatchEvent(new Event("model-services-changed"));
    } catch (e) {
      if (mounted.current)
        setNotice(
          `原操作结果尚未确认；没有重新验证模型。${e instanceof Error ? e.message : String(e)}`,
        );
    } finally {
      running.current = false;
      if (mounted.current) setBusy(null);
    }
  };
  if (!pending.requests.length && !pending.invalid && !notice) return null;
  return (
    <section aria-label="模型服务操作核查" className="space-y-3">
      {pending.invalid > 0 && (
        <Alert status="warning">
          <AlertTitle>操作恢复记录不可读取</AlertTitle>
          <AlertDescription>
            保留了不可读取的记录，没有自动删除或重放操作。请核对本机诊断与已提交目录。
          </AlertDescription>
        </Alert>
      )}
      {pending.requests.map((request) => (
        <Alert status="warning" key={request.operationId}>
          <AlertTitle>
            模型服务{request.action.kind === "save" ? "发布" : "删除"}结果待核查
          </AlertTitle>
          <AlertDescription>
            <p className="break-all">操作编号：{request.operationId}</p>
            <p>恢复仅查询结果；按原编号重试会提交原参数，不重新验证模型。</p>
            <div className="mt-2 flex flex-wrap gap-2">
              <Button
                variant="tertiary"
                disabled={busy !== null}
                onClick={() => void check(request, false)}
              >
                核查已保存的操作
              </Button>
              <Button
                variant="tertiary"
                disabled={busy !== null}
                onClick={() => void check(request, true)}
              >
                重试已保存的原操作
              </Button>
            </div>
          </AlertDescription>
        </Alert>
      ))}
      {notice && (
        <p role="status" className="text-body text-fg-muted">
          {notice}
        </p>
      )}
    </section>
  );
}
