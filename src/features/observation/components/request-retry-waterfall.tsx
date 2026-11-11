import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { recordAgentKey } from "@/shared/types/agent";
import { sessionLink } from "@/features/sessions/display";
import { durationLabel, requestIssues } from "../request-display";
import type { RequestRecord } from "../request-types";

const measured = (ms: number | null | undefined): ms is number =>
  typeof ms === "number" && Number.isFinite(ms) && ms >= 0;

const outcomes: Record<
  string,
  { label: string; status: "success" | "danger" | "neutral" }
> = {
  success: { label: "成功", status: "success" },
  error: { label: "失败", status: "danger" },
  failed: { label: "失败", status: "danger" },
  incomplete: { label: "响应不完整", status: "danger" },
  in_progress: { label: "进行中", status: "neutral" },
  cancelled: { label: "已取消", status: "neutral" },
};

export function RequestRetryWaterfall({
  request,
  returnTo,
}: {
  request: RequestRecord;
  returnTo: string;
}) {
  const { attempts, nativeLinks } = request;
  const origin = attempts[0];
  const rows = attempts.map((attempt, index) => {
    const tick = attempt.startTickMs;
    const offset =
      index === 0
        ? 0
        : origin?.clockId &&
            attempt.clockId === origin.clockId &&
            measured(origin.startTickMs) &&
            measured(tick) &&
            tick >= origin.startTickMs
          ? tick - origin.startTickMs
          : null;
    const duration = measured(attempt.durationMs) ? attempt.durationMs : null;
    const end =
      offset !== null && duration !== null && measured(offset + duration)
        ? offset + duration
        : null;
    const links = nativeLinks?.filter(
      (link) =>
        link.attemptId === attempt.id &&
        link.responseId &&
        link.sessionId &&
        link.eventId,
    );
    return {
      attempt,
      offset,
      end,
      duration,
      link: links?.length === 1 ? links[0] : undefined,
    };
  });
  const max = rows.reduce((max, row) => Math.max(max, row.end ?? 0), 0);
  const hasSpan = rows.some((row) => row.end !== null);
  return (
    <section
      aria-label="重试瀑布图"
      className="min-w-0 space-y-3 rounded-xl border border-border p-4"
    >
      <h3 className="text-title font-medium">重试瀑布图</h3>
      {hasSpan && (
        <div
          aria-hidden="true"
          className="flex justify-between text-label tabular-nums text-fg-muted"
        >
          <span>0 秒</span>
          {max > 0 && <span>{durationLabel(max)}</span>}
        </div>
      )}
      <ol className="space-y-3">
        {rows.map(({ attempt, offset, end, duration, link }, index) => {
          const outcome = Object.hasOwn(outcomes, attempt.status)
            ? outcomes[attempt.status]
            : undefined;
          return (
            <li
              key={attempt.id}
              className="space-y-2 border-t border-border pt-3"
            >
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                <span className="min-w-0 wrap-anywhere font-medium">
                  尝试 {index + 1} · {attempt.provider}
                </span>
                <Badge
                  size="sm"
                  variant="plain"
                  status={outcome?.status ?? "neutral"}
                >
                  {outcome?.label ?? "结果未知"}
                </Badge>
                {attempt.httpStatus != null && (
                  <span className="text-label text-fg-muted">
                    HTTP {attempt.httpStatus}
                  </span>
                )}
                {attempt.issue && (
                  <span className="wrap-anywhere text-label text-fg-muted">
                    {requestIssues[attempt.issue] ?? attempt.issue}
                  </span>
                )}
              </div>
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-label tabular-nums text-fg-muted">
                <span>
                  耗时 {duration === null ? "未知" : durationLabel(duration)}
                </span>
                {index > 0 && (
                  <span>
                    与上次间隔{" "}
                    {measured(attempt.backoffMs)
                      ? durationLabel(attempt.backoffMs)
                      : "未知"}
                  </span>
                )}
                {offset === null && <span>相对起点未知</span>}
                {link && (
                  <Button
                    variant="link"
                    size="sm"
                    className="px-0"
                    onClick={() => {
                      window.location.hash = sessionLink(link.sessionId, {
                        agent: recordAgentKey(attempt),
                        anchor: link.eventId,
                        view: "timeline",
                        returnTo,
                      });
                    }}
                  >
                    定位会话调用
                  </Button>
                )}
              </div>
              {end !== null && offset !== null && (
                <div
                  className="relative h-2 rounded-full bg-muted"
                  role="img"
                  aria-label={`从 ${durationLabel(offset)} 到 ${durationLabel(end)}`}
                >
                  <div
                    className={`absolute inset-y-0 rounded-full ${outcome?.status === "danger" ? "bg-fg-danger" : "bg-fg-brand"}`}
                    style={{
                      insetInlineStart:
                        duration === 0 && offset === max && max > 0
                          ? "calc(100% - 1px)"
                          : `${max > 0 ? (offset / max) * 100 : 0}%`,
                      width:
                        duration === 0 ? 1 : `${((end - offset) / max) * 100}%`,
                    }}
                  />
                </div>
              )}
            </li>
          );
        })}
      </ol>
    </section>
  );
}
