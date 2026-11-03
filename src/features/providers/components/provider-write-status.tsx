import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import type { useProviderWrite } from "../hooks/use-provider-write";
export function ProviderWriteStatus({
  write,
}: {
  write: ReturnType<typeof useProviderWrite>;
}) {
  if (write.state.kind === "cancelled")
    return (
      <p role="status" className="text-body text-fg-muted">
        原操作已取消，后续结果不会保存。
      </p>
    );
  if (write.state.kind !== "unknown")
    return write.busy ? (
      <p role="status" className="text-body text-fg-muted">
        {"message" in write.state ? write.state.message : "正在核对来源…"}
      </p>
    ) : null;
  const request = write.state.request;
  const catalog = write.state.catalog;
  return (
    <Alert status="warning" className="mt-4">
      <AlertTitle>保存结果待核查</AlertTitle>
      <AlertDescription>
        <p>{write.state.message}</p>
        <p className="mt-2">
          操作：{write.state.request.operationId}
          。关闭页面后仍可在服务商恢复列表核查。
        </p>
        <div className="mt-3 flex flex-wrap gap-2">
          <Button
            type="button"
            variant="tertiary"
            disabled={write.busy}
            onClick={() => void write.resume("check", request, catalog)}
          >
            核查原结果
          </Button>
          <Button
            type="button"
            variant="tertiary"
            disabled={write.busy}
            onClick={() => void write.resume("retry", request, catalog)}
          >
            重试原请求
          </Button>
          <Button
            type="button"
            variant="tertiary"
            disabled={write.busy}
            onClick={() => void write.resume("cancel", request, catalog)}
          >
            取消原操作
          </Button>
        </div>
        <p className="mt-2">
          重试使用相同编号与输入；若请求尚未开始，连接验证可能产生费用。取消阻止后续保存，不撤回已经发出的远端请求。
        </p>
      </AlertDescription>
    </Alert>
  );
}
