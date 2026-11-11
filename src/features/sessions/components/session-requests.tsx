import { useState } from "react";
import { Button } from "@/components/ui/button";
import { PageActions, PageHeader } from "@/components/ui/page-layout";
import { observationApi } from "@/features/observation/api";
import { RequestsTable } from "@/features/observation/components/requests-table";
import { requestParams } from "@/features/observation/request-display";
import type { RequestQuery } from "@/features/observation/request-types";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import { createReadResourceFamily } from "@/shared/lib/read-resource";
import { number, requestReturnPath } from "../display";

const requestPages = createReadResourceFamily(
  (query: RequestQuery) => observationApi.requestDashboard(query),
  { interval: false },
);

export function SessionRequests({
  id,
  title,
  returnTo,
  embedded = false,
}: {
  id: string;
  title?: string;
  returnTo?: string;
  embedded?: boolean;
}) {
  const [pages, setPages] = useState<
    Pick<RequestQuery, "snapshot" | "cursor">[]
  >([{}]);
  const query: RequestQuery = {
    session: id,
    source: "gateway",
    ...pages[pages.length - 1],
  };
  const resource = useSharedQuery(requestPages, query);
  const page = resource.data;
  const back = requestReturnPath(returnTo);
  return (
    <section
      aria-label="会话模型请求"
      className="min-h-0 flex-1 overflow-y-auto p-4"
    >
      <PageHeader className="mb-3 px-0">
        {embedded ? (
          <p className="text-label text-fg-muted">
            {page ? `共 ${number(page.total)} 条网关请求` : "网关请求"}
          </p>
        ) : (
          <h2 className="min-w-0 break-words text-title font-medium">
            {title ?? page?.items[0]?.sessionTitle ?? "会话模型请求"}
          </h2>
        )}
        <PageActions>
          {!embedded && back && (
            <Button
              variant="tertiary"
              size="sm"
              onClick={() => {
                window.location.hash = back;
              }}
            >
              返回来源请求
            </Button>
          )}
          <Button
            variant="tertiary"
            size="sm"
            loading={resource.loading}
            onClick={() => {
              if (!query.snapshot) void resource.invalidate();
              else setPages([{}]);
            }}
          >
            刷新请求
          </Button>
        </PageActions>
      </PageHeader>
      {!embedded && (
        <div className="mb-3 flex flex-wrap items-center gap-2 text-label text-fg-muted">
          <p>对话正文需授权读取。</p>
          <Button asChild variant="link" size="sm">
            <a href="#/agents">管理读取范围</a>
          </Button>
        </div>
      )}
      {resource.error ? (
        <p role="alert" className="text-body text-fg-muted">
          {resource.error}
        </p>
      ) : !page ? (
        <p role="status" className="text-body text-fg-muted">
          正在读取会话请求…
        </p>
      ) : page.items.length === 0 ? (
        <p className="text-body text-fg-muted">
          这段会话还没有可关联的模型请求。
        </p>
      ) : (
        <>
          <RequestsTable
            rows={page.items}
            onOpen={(requestId) => {
              const search = requestParams(
                page.query,
                "all",
                page.snapshot,
                query.cursor,
              );
              window.location.hash = `/requests/${encodeURIComponent(requestId)}?${search}`;
            }}
          />
          <div className="mt-3 flex flex-wrap items-center gap-2">
            <Button
              variant="tertiary"
              size="sm"
              disabled={resource.loading || pages.length < 2}
              onClick={() => setPages((old) => old.slice(0, -1))}
            >
              上一页请求
            </Button>
            <Button
              variant="tertiary"
              size="sm"
              disabled={resource.loading || !page.nextCursor}
              onClick={() => {
                const cursor = page.nextCursor;
                if (cursor)
                  setPages((old) => [
                    ...old.map((p) => ({ ...p, snapshot: page.snapshot })),
                    { snapshot: page.snapshot, cursor },
                  ]);
              }}
            >
              下一页请求
            </Button>
            {!embedded && (
              <span className="text-label text-fg-muted">
                共 {number(page.total)} 条请求
              </span>
            )}
          </div>
        </>
      )}
    </section>
  );
}
