import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import type { NativeImportRequest } from "../native-import-operation";
import type { ProviderImportCandidate } from "../types";

export function NativeImportRecovery({
  requests,
  conflict,
  message,
  saving,
  loading,
  confirmed,
  sourceFor,
  onConfirm,
  onSource,
  onAction,
}: {
  requests: NativeImportRequest[];
  conflict: boolean;
  message: string | null;
  saving: boolean;
  loading: boolean;
  confirmed: string | null;
  sourceFor: (
    request: NativeImportRequest,
  ) => ProviderImportCandidate | undefined;
  onConfirm: (value: string | null) => void;
  onSource: () => void;
  onAction: (
    request: NativeImportRequest,
    action: "resume" | "query" | "cancel",
  ) => void;
}) {
  return (
    <div className="space-y-3">
      {conflict && (
        <Alert status="warning">
          <AlertTitle>导入记录需要核查</AlertTitle>
          <AlertDescription>
            暂存记录不可读取或与原后台记录不一致，原字节已保留，新的导入暂不可提交。
          </AlertDescription>
        </Alert>
      )}
      {message && (
        <p role="status" className="text-body text-fg-muted">
          {message}
        </p>
      )}
      {requests.length > 0 && (
        <Alert status="warning">
          <AlertTitle>有待核查的本机配置导入</AlertTitle>
          <AlertDescription>
            <div className="space-y-4">
              {requests.map((request) => {
                const source = sourceFor(request),
                  id = request.operation.operationId;
                return (
                  <div
                    key={id}
                    className="space-y-2"
                    data-testid="native-import-recovery"
                  >
                    <p className="break-all">原操作：{id}</p>
                    <p>
                      模型：{request.model ?? source?.model ?? "原配置默认模型"}
                      。取消导入不会删除已写入的凭据。
                    </p>
                    {source ? (
                      <div className="space-y-1">
                        <p>原来源：{source.name}</p>
                        <p className="break-all">
                          配置文件：{source.sourcePath}
                        </p>
                        <p className="break-all">
                          Endpoint：{source.endpoint ?? "未识别"}
                        </p>
                        <p>接口协议：{source.protocol}</p>
                      </div>
                    ) : (
                      <p>
                        请连接原后台并重新识别原来源；来源变更后无法直接继续。
                      </p>
                    )}
                    <div className="flex flex-wrap gap-2">
                      <Button
                        size="sm"
                        variant="tertiary"
                        disabled={saving}
                        onClick={() => onAction(request, "query")}
                      >
                        查询原导入结果
                      </Button>
                      <Button
                        size="sm"
                        variant="tertiary"
                        disabled={saving || loading}
                        onClick={onSource}
                      >
                        核对原来源
                      </Button>
                      <Button
                        size="sm"
                        variant="tertiary"
                        disabled={saving || conflict}
                        onClick={() => onAction(request, "cancel")}
                      >
                        取消这次导入
                      </Button>
                    </div>
                    {source && (
                      <>
                        <label className="flex items-center gap-2">
                          <Checkbox
                            aria-label="已核对原导入来源"
                            disabled={saving || conflict}
                            checked={confirmed === id}
                            onCheckedChange={(value) =>
                              onConfirm(value === true ? id : null)
                            }
                          />
                          <span>我已核对原来源和模型，继续这次导入</span>
                        </label>
                        <Button
                          size="sm"
                          variant="neutral"
                          disabled={saving || conflict || confirmed !== id}
                          onClick={() => onAction(request, "resume")}
                        >
                          继续原导入
                        </Button>
                      </>
                    )}
                  </div>
                );
              })}
            </div>
          </AlertDescription>
        </Alert>
      )}
    </div>
  );
}
