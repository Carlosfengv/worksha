import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Badge } from "@/components/ui/badge";
import { date, number } from "../display";
import type { ContextRecord, SessionContext } from "../types";

function Records({ rows, empty }: { rows: ContextRecord[]; empty: string }) {
  const { agentNames } = useAgentDirectory();
  return rows.length ? (
    <Accordion>
      {rows.map((r) => (
        <AccordionItem key={r.id} value={r.id}>
          <AccordionTrigger>{r.label}</AccordionTrigger>
          <AccordionContent>
            <div className="space-y-2">
              <p className="text-label text-fg-muted">
                真实采集 · {date(r.timestamp)} ·{" "}
                {r.agentProductId
                  ? (agentNames[r.agentProductId] ?? r.agentProductId)
                  : "客户端来源未知"}{" "}
                · 原生位置 {r.sourceLine || "未提供"}
              </p>
              <p className="text-label text-fg-muted">
                {r.kind === "read"
                  ? "已记录读取成功，当前是否仍在上下文中未知。"
                  : r.kind === "injected"
                    ? "原生记录中的注入内容，当前是否保留未知。"
                    : "摘要与边界按原生记录展示，不推测当前保留内容。"}
              </p>
              {r.truncated && (
                <p className="text-label text-fg-warning">内容已截断。</p>
              )}
              <pre className="max-h-80 overflow-auto whitespace-pre-wrap break-words text-code">
                {r.text ?? "原生记录未提供可读正文"}
              </pre>
            </div>
          </AccordionContent>
        </AccordionItem>
      ))}
    </Accordion>
  ) : (
    <p className="text-body text-fg-muted">{empty}</p>
  );
}
export function SessionContextPanel({
  context,
  partial,
}: {
  context?: SessionContext;
  partial: boolean;
}) {
  const { agentNames } = useAgentDirectory();
  return (
    <section className="space-y-5" aria-label="会话上下文">
      <h3 className="text-title font-medium">上下文占用</h3>
      <dl className="grid gap-3 sm:grid-cols-3">
        {[
          [
            "最近请求输入",
            context?.inputTokens == null
              ? "未知"
              : `${number(context.inputTokens)} Token`,
            context?.inputTokens == null ? "未知" : "真实采集",
          ],
          [
            "原生窗口上限",
            context?.windowTokens == null
              ? "未知"
              : `${number(context.windowTokens)} Token`,
            context?.windowTokens == null ? "未知" : "真实采集",
          ],
          [
            "最近输入占比",
            context?.usedPercentage == null
              ? "未知"
              : `${context.usedPercentage.toFixed(1)}%`,
            context?.usedPercentage == null ? "未知" : "估算",
          ],
        ].map(([label, value, quality]) => (
          <div
            key={label}
            className="space-y-2 rounded-xl border border-border p-3"
          >
            <dt className="text-label text-fg-muted">{label}</dt>
            <dd className="text-title">{value}</dd>
            <Badge status="neutral">{quality}</Badge>
          </div>
        ))}
      </dl>
      <p className="text-label text-fg-muted">
        {context?.source ?? "尚无原生上下文用量样本"}
        {context?.source
          ? ` · ${context.sampleAgentProductId ? (agentNames[context.sampleAgentProductId] ?? context.sampleAgentProductId) : "客户端来源未知"}`
          : ""}
        {context?.sampleAt ? ` · ${date(context.sampleAt)}` : ""}
        {context?.model ? ` · ${context.model}` : ""}。输入占比按最近输入 /
        原生窗口计算，不代表此刻精确占用；会话累计 Token 不参与。
      </p>
      {partial && (
        <p role="status" className="text-label text-fg-warning">
          会话为部分读取快照，缺失内容保留未知。
        </p>
      )}
      {context?.limited && (
        <p className="text-label text-fg-warning">
          记录较多，以下各保留最近 50 条。
        </p>
      )}
      <h3 className="text-title font-medium">
        压缩记录 · {number(context?.compactionCount)}
      </h3>
      <Records
        rows={context?.compactions ?? []}
        empty="未发现明确的压缩记录，是否发生过压缩未知。"
      />
      <h3 className="text-title font-medium">
        已加载内容与读取记录 · {number(context?.loadedCount)}
      </h3>
      <p className="text-label text-fg-muted">
        只展示明确注入或读取成功的证据；发现文件、发起工具调用不代表加载。未记录内容与当前保留状态均为未知。
      </p>
      <Records rows={context?.loaded ?? []} empty="尚无可确认的加载记录。" />
    </section>
  );
}
