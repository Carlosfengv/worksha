import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import { iamResource } from "./queries";

export function IamResourceDetail({
  id,
  onClose,
}: {
  id: string;
  onClose: () => void;
}) {
  const result = useSharedQuery(iamResource, id);
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            {(!result.error && result.data?.name) || "模型服务详情"}
          </DialogTitle>
          <DialogDescription>模型服务的只读摘要</DialogDescription>
        </DialogHeader>
        {result.error ? (
          <Alert status="danger">
            <AlertDescription>{result.error}</AlertDescription>
          </Alert>
        ) : result.data ? (
          <dl className="space-y-3 text-body">
            <div className="space-y-1">
              <dt className="text-label text-fg-muted">创建者</dt>
              <dd>{result.data.creator}</dd>
            </div>
            <div className="space-y-1">
              <dt className="text-label text-fg-muted">模型</dt>
              <dd>
                <ul className="list-disc pl-5">
                  {result.data.models.map((name, index) => (
                    <li key={index}>{name}</li>
                  ))}
                </ul>
              </dd>
            </div>
          </dl>
        ) : (
          <p role="status">正在读取资源详情…</p>
        )}
      </DialogContent>
    </Dialog>
  );
}
