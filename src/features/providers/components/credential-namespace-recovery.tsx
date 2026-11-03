import { useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { useCredentialOperations } from "../hooks/use-credential-operations";
import type { CredentialInitialization } from "../credential-receipt";
import { message } from "../display";

export function CredentialNamespaceRecovery({
  operations,
  disabled,
}: {
  operations: ReturnType<typeof useCredentialOperations>;
  disabled: boolean;
}) {
  const [request, setRequest] = useState<CredentialInitialization | null>(null),
    [error, setError] = useState<string | null>(null);
  const namespace = operations.namespace;
  const unverified = namespace?.namespaceState === "unverified";
  const legacyUnverified = namespace?.legacyState === "unverified";
  if (
    !unverified &&
    !legacyUnverified &&
    !operations.namespaceError &&
    !operations.loadingNamespace
  )
    return null;
  const choose = (choice: CredentialInitialization["choice"]) => {
    if (!namespace) return;
    setError(null);
    setRequest({
      storeId: namespace.storeId,
      namespaceId: namespace.namespaceId,
      choice,
    });
  };
  return (
    <>
      <Alert status={unverified || legacyUnverified ? "warning" : undefined}>
        <AlertTitle>
          {unverified
            ? "凭据恢复空间尚未核查"
            : legacyUnverified
              ? "旧凭据恢复材料尚未核查"
              : "凭据恢复状态"}
        </AlertTitle>
        <AlertDescription className="space-y-3">
          {operations.loadingNamespace && (
            <p role="status">正在读取凭据恢复状态…</p>
          )}
          {(unverified || legacyUnverified) && (
            <p>
              旧记录和密钥会保留。请先核查原恢复材料；如果选择建立新空间，旧请求不会自动转入，也不会重新保存旧密钥。
            </p>
          )}
          {operations.namespaceError && <p>{operations.namespaceError}</p>}
          <div className="flex flex-wrap gap-2">
            {(unverified || legacyUnverified) && (
              <Button
                type="button"
                variant="secondary"
                disabled={disabled || operations.busy}
                onClick={() => choose("restore")}
              >
                核查原恢复材料
              </Button>
            )}
            {unverified && (
              <Button
                type="button"
                variant="secondary"
                disabled={disabled || operations.busy}
                onClick={() => choose("new")}
              >
                建立新的恢复空间
              </Button>
            )}
            <Button
              type="button"
              variant="ghost"
              disabled={disabled || operations.busy}
              onClick={() => {
                setError(null);
                void operations
                  .refreshNamespace()
                  .catch((reason) => setError(message(reason)));
              }}
            >
              重新读取恢复状态
            </Button>
          </div>
          {error && !request && <p role="status">{error}</p>}
        </AlertDescription>
      </Alert>
      <Dialog
        open={!!request}
        onOpenChange={(open) => {
          if (!open) setRequest(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {request?.choice === "restore"
                ? "核查并恢复原凭据记录"
                : "确认建立新的凭据恢复空间"}
            </DialogTitle>
            <DialogDescription>
              {request?.choice === "restore"
                ? "只核查原后台对应的完整恢复材料，将记录保存到本机数据库；不会保存或删除任何密钥。材料缺失或来源不符时保留现状。"
                : "旧恢复材料尚未核查。此操作只启用新空间；旧记录和密钥保留，旧请求不会自动转入。若需找回旧操作，请先恢复原目录或数据库备份。"}
            </DialogDescription>
          </DialogHeader>
          {error && <p role="status">{error}</p>}
          <DialogFooter>
            <Button
              type="button"
              variant="secondary"
              onClick={() => setRequest(null)}
            >
              暂不处理
            </Button>
            <Button
              type="button"
              disabled={
                disabled ||
                operations.busy ||
                !namespace ||
                request?.namespaceId !== namespace.namespaceId ||
                request?.storeId !== namespace.storeId
              }
              onClick={() => {
                if (!request) return;
                setError(null);
                void operations
                  .initializeNamespace(request)
                  .then(() => setRequest(null))
                  .catch((reason) => setError(message(reason)));
              }}
            >
              {request?.choice === "restore"
                ? "确认核查原材料"
                : "确认建立新空间"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
