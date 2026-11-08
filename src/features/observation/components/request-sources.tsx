import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { Tabs, TabsList, TabItem } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import {
  Accordion,
  AccordionItem,
  AccordionTrigger,
  AccordionContent,
} from "@/components/ui/accordion";
import { date, number } from "@/features/sessions/display";
import { compactNumber } from "@/shared/lib/format";
import type { SourceReport } from "../request-types";
const states: Record<string, string> = {
  disabled: "未授权",
  pending: "等待索引",
  scanning: "扫描中",
  ready: "可读取",
  available: "可读取",
  partial: "部分可读取",
  not_found: "未发现记录",
  permission_denied: "无权限",
  error: "读取失败",
  unsupported: "版本不支持",
  empty: "暂无记录",
};
export function RequestSources({
  source,
  report,
  onChange,
}: {
  source: string;
  report?: SourceReport | null;
  onChange: (source: string) => void;
}) {
  const { familyNames } = useAgentDirectory();
  return (
    <section aria-label="来源与跨源统计" className="min-w-0 space-y-3">
      <Tabs
        value={source}
        onValueChange={onChange}
        variant="segment"
        color="default"
      >
        <TabsList aria-label="请求数据来源">
          <TabItem value="gateway" label="网关请求" />
          <TabItem value="native" label="原生响应" />
          <TabItem value="linked" label="跨来源精确匹配" />
        </TabsList>
      </Tabs>
      {report && (
        <div
          className="flex flex-wrap gap-x-4 gap-y-1 text-label text-fg-muted"
          aria-label="来源摘要"
        >
          <span title={number(report.gateway.requests)}>
            网关 {compactNumber(report.gateway.requests)}
          </span>
          <span title={number(report.native.requests)}>
            原生响应 {compactNumber(report.native.requests)}
          </span>
          <span title={number(report.linked.requests)}>
            精确匹配 {compactNumber(report.linked.requests)}
          </span>
          <span>各来源独立统计，不直接相加</span>
        </div>
      )}
      {source === "linked" && (
        <p className="text-label text-fg-muted">
          按响应 ID
          匹配网关请求与原生响应。请求能够打开所属会话时，仍可能缺少用于跨来源匹配的响应
          ID。
        </p>
      )}
      {report?.collectors.length &&
      !report.collectors.some((c) => c.enabled) ? (
        <p role="status" className="text-label text-fg-muted">
          原生来源尚未授权。可在 Agents 的连接与观测中选择允许读取的来源。
        </p>
      ) : null}
      {report?.collectors
        .filter(
          (c) =>
            c.enabled &&
            (c.error ||
              ["partial", "error", "permission_denied", "unsupported"].includes(
                c.status,
              )),
        )
        .map((c) => (
          <p
            key={c.agentId}
            role="status"
            className="break-words text-label text-fg-muted"
          >
            {familyNames[c.agentId] ?? c.agentId} ·{" "}
            {states[c.status] ?? c.status}
            {c.error ? `：${c.error}` : ""}
          </p>
        ))}
      {report && (
        <Accordion className="w-full" type="multiple">
          <AccordionItem value="sources">
            <AccordionTrigger>跨来源统计与采集状态</AccordionTrigger>
            <AccordionContent>
              <div className="space-y-3">
                <div className="grid gap-3 md:grid-cols-3">
                  {[
                    {
                      label: "网关逻辑请求",
                      value: report.gateway.requests,
                      note: `${report.gateway.attempts} 次尝试`,
                      tokens: report.gateway.usage.totalTokens,
                    },
                    {
                      label: "原生模型响应记录",
                      value: report.native.requests,
                      note: "来自已授权记录，可能经过网关",
                      tokens: report.native.usage.totalTokens,
                    },
                    {
                      label: "与原生响应精确匹配的网关请求",
                      value: report.linked.requests,
                      note: `对应 ${report.matchedNative} 条原生响应，消费只计一份`,
                      tokens: report.linked.usage.totalTokens,
                    },
                  ].map((c) => (
                    <article
                      key={c.label}
                      className="min-w-0 rounded-xl border border-border p-3"
                    >
                      <p className="text-label text-fg-muted">{c.label}</p>
                      <p className="my-1 text-title font-medium tabular-nums">
                        {number(c.value)}
                      </p>
                      <p className="text-label text-fg-muted">
                        {number(c.tokens)} Token · {c.note}
                      </p>
                    </article>
                  ))}
                </div>
                <p className="text-label text-fg-muted">
                  {report.unlinkedNative} 条原生响应重叠未排除 ·{" "}
                  {report.ambiguousNative} 条身份冲突或多候选 ·{" "}
                  {report.usageConflicts} 条用量冲突 ·{" "}
                  {report.nativeMissingTime} 条缺少时间。{report.scope}
                </p>
                <div className="flex flex-wrap gap-3 text-label text-fg-muted">
                  {report.collectors.map((c) => (
                    <span key={c.agentId}>
                      {familyNames[c.agentId] ?? c.agentId}：
                      {!c.enabled ? "未授权" : (states[c.status] ?? c.status)} ·{" "}
                      {date(c.asOf)}
                      {c.skippedFiles
                        ? ` · ${c.skippedFiles} 个文件未读取`
                        : ""}
                      {c.error ? ` · ${c.error}` : ""}
                    </span>
                  ))}
                </div>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => {
                    window.location.hash = "/agents";
                  }}
                >
                  管理来源授权
                </Button>
              </div>
            </AccordionContent>
          </AccordionItem>
        </Accordion>
      )}
    </section>
  );
}
