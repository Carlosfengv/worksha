import { useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import { createReadResourceFamily } from "@/shared/lib/read-resource";
import { observationApi } from "../api";
import { requestParams } from "../request-display";
import type { RequestQuery } from "../request-types";
import { RequestsTable } from "./requests-table";

const requestPages = createReadResourceFamily((query: RequestQuery) =>
  observationApi.requestDashboard(query),
);

export function ClientRequestAudit({ clientId }: { clientId: string }) {
  const [position, setPosition] = useState<{
    snapshot: string;
    cursor: string;
  }>();
  const [exportState, setExportState] = useState({ busy: false, message: "" });
  const query = useMemo<RequestQuery>(
    () => ({
      source: "gateway",
      client: clientId,
      clientScope: "lan",
      ...position,
    }),
    [clientId, position],
  );
  const { data, error, loading, reload } = useSharedQuery(requestPages, query);
  return (
    <section aria-label="密钥请求审计" className="space-y-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="font-mono text-label">client:{clientId}</p>
        <Button
          size="sm"
          variant="secondary"
          disabled={!data || !!error || exportState.busy}
          onClick={async () => {
            if (!data) return;
            setExportState({ busy: true, message: "" });
            try {
              const result = await observationApi.exportRequests(data.snapshot);
              setExportState({
                busy: false,
                message:
                  result.status === "saved"
                    ? `已导出 ${result.count} 条请求`
                    : "",
              });
            } catch (reason) {
              setExportState({
                busy: false,
                message:
                  reason instanceof Error ? reason.message : String(reason),
              });
            }
          }}
        >
          导出此密钥 CSV
        </Button>
      </div>
      {!!exportState.message && <p role="status">{exportState.message}</p>}
      {error && (
        <p role="alert">
          {error}{" "}
          <Button size="sm" variant="tertiary" onClick={reload}>
            重试
          </Button>
        </p>
      )}
      {loading && !data && <p role="status">正在读取密钥请求…</p>}
      {data && !error && (
        <>
          <RequestsTable
            rows={data.items}
            requestHref={(id) =>
              `#/requests/${encodeURIComponent(id)}?${requestParams(query, "all")}`
            }
          />
          <div className="flex justify-end gap-2">
            {position && (
              <Button
                variant="tertiary"
                size="sm"
                onClick={() => setPosition(undefined)}
              >
                返回第一页
              </Button>
            )}
            {data.nextCursor && (
              <Button
                variant="tertiary"
                size="sm"
                onClick={() =>
                  setPosition({
                    snapshot: data.snapshot,
                    cursor: data.nextCursor!,
                  })
                }
              >
                下一页请求
              </Button>
            )}
          </div>
        </>
      )}
    </section>
  );
}
