import { useId, useState } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { CredentialRecoveryRecord } from "../credential-recovery-records";
import type { LegacyCredentialPreview } from "../legacy-credential-receipt";
import type { useCredentialOperations } from "../hooks/use-credential-operations";
import { message } from "../display";
export function LegacyCredentialRecovery({
  record,
  operations,
  disabled,
}: {
  record: CredentialRecoveryRecord;
  operations: ReturnType<typeof useCredentialOperations>;
  disabled: boolean;
}) {
  const [preview, setPreview] = useState<LegacyCredentialPreview | null>(null),
    [confirmed, setConfirmed] = useState(false),
    [error, setError] = useState<string | null>(null);
  const consentId = useId();
  const matches =
    !!preview &&
    preview.storeId === operations.storeId &&
    preview.namespaceId === operations.namespace?.namespaceId;
  return (
    <>
      <Button
        type="button"
        size="sm"
        variant="secondary"
        disabled={
          disabled || operations.busy || !operations.canRecoverLegacy(record)
        }
        onClick={() => {
          setError(null);
          void operations
            .previewLegacy(record)
            .then((value) => {
              setPreview(value);
              setConfirmed(false);
            })
            .catch((reason) => setError(message(reason)));
        }}
      >
        {record.legacySourceDigest ? "继续旧凭据恢复" : "核查旧记录来源"}
      </Button>
      {error && !preview && <p role="status">{error}</p>}
      <Dialog
        open={!!preview}
        onOpenChange={(open) => {
          if (!open && !operations.busy) setPreview(null);
        }}
      >
        <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto">
          <DialogHeader>
            <DialogTitle>确认旧凭据的恢复来源</DialogTitle>
            <DialogDescription>
              旧版记录没有后台身份。请先确认它属于当前连接的后台，不能只凭名称判断。确认后会保留原材料和原引用，并停止旧入口继续修改这条记录。
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3 text-body text-fg-muted">
            <p>凭据名称：{preview?.label ?? record.label}</p>
            <p>
              {preview?.stage === "unclaimed"
                ? "本次核查尚未迁移，也未读取或保存密钥。"
                : preview?.stage === "committed"
                  ? "后台已记录迁移结果。本次只恢复本机提示，再核查真实凭据；不会重新创建。"
                  : "后台已保留恢复意图。请沿用原来源继续恢复，原请求不会转到其他后台。"}
            </p>
            <p>
              迁移不会重新保存 API Key
              或发送模型请求；仍待确认的写入会保留原阶段。必要时只新增保护恢复记录的辅助密钥。完成后还需核查凭据，才能明确采用到连接。
            </p>
            <label
              htmlFor={consentId}
              className="flex items-start gap-3 text-fg-default"
            >
              <Checkbox
                id={consentId}
                checked={confirmed}
                onCheckedChange={(value) => setConfirmed(value)}
                disabled={disabled || operations.busy || !matches}
              />
              <span>
                我确认这份旧记录属于当前连接的后台，同意保留原材料并按原引用恢复。
              </span>
            </label>
            {error && <p role="status">{error}</p>}
            {!matches && preview && (
              <p role="status">
                当前后台或恢复空间已变化，请保留原记录后重新核查。
              </p>
            )}
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="secondary"
              disabled={operations.busy}
              onClick={() => setPreview(null)}
            >
              暂不处理
            </Button>
            <Button
              type="button"
              disabled={disabled || operations.busy || !confirmed || !matches}
              onClick={() => {
                if (!preview) return;
                setError(null);
                void operations
                  .confirmLegacy(record, preview)
                  .then(() => setPreview(null))
                  .catch((reason) => setError(message(reason)));
              }}
            >
              确认来源并恢复
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
