import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import type { useServiceWrite } from "../hooks/use-service-write";
export function ServiceWriteFeedback({
  operation,
}: {
  operation: ReturnType<typeof useServiceWrite>;
}) {
  const { state, busy } = operation;
  if (state.kind === "failed")
    return (
      <Alert status="danger" role="alert">
        <AlertTitle>操作未提交</AlertTitle>
        <AlertDescription>{state.message}</AlertDescription>
      </Alert>
    );
  if (
    state.kind === "unknown" ||
    state.kind === "checking" ||
    state.kind === "submitting"
  )
    return (
      <Alert status="warning" role="status">
        <AlertTitle>操作结果待核查</AlertTitle>
        <AlertDescription>
          <p>{state.message}</p>
          <p className="break-all">操作编号：{state.request.operationId}</p>
          <div className="mt-2 flex flex-wrap gap-2">
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => void operation.check()}
            >
              查询原操作结果
            </Button>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => void operation.check(true)}
            >
              按原编号重试
            </Button>
          </div>
        </AlertDescription>
      </Alert>
    );
  return null;
}
