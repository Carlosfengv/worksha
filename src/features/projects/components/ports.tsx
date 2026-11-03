import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import {
  DetailList,
  DetailListItem,
  DetailListLabel,
  DetailListValue,
} from "@/components/ui/detail-list";
import type { ProjectDetail } from "@/features/projects/types";
import { date } from "@/features/sessions/display";

export function Ports({
  detail,
  anchor,
}: {
  detail: ProjectDetail;
  anchor?: string;
}) {
  const ports = detail.listeners;
  return (
    <>
      <h3 className="text-title font-medium">当前 TCP LISTEN</h3>
      <p className="text-label text-fg-muted">
        {date(ports.asOf)} · 当前用户可见范围 ·{" "}
        {ports.state === "ready"
          ? "采集完成"
          : ports.state === "stale"
            ? "最后快照已过期"
            : "部分覆盖 / 未采集"}
        。监听不表示健康检查通过；进程参数和环境变量不读取。
      </p>
      {ports.error && (
        <p role="status" className="text-body text-fg-muted">
          {ports.error}
        </p>
      )}
      {ports.items.length ? (
        <Accordion
          className="w-full"
          type="multiple"
          defaultValue={anchor ? [anchor] : []}
        >
          <div className="space-y-2">
            {ports.items.map((port) => (
              <AccordionItem key={port.id} value={port.id}>
                <AccordionTrigger>
                  {port.commandRedacted} · {port.address}:{port.port} · PID{" "}
                  {port.pid}
                  {anchor === port.id ? " · 已定位" : ""}
                </AccordionTrigger>
                <AccordionContent>
                  <div className="space-y-3 p-3">
                    <DetailList>
                      {[
                        ["协议", port.protocol],
                        ["进程开始", date(port.startedAt)],
                        ["进程身份", port.processIdentity],
                        ["工作目录", port.cwd],
                        ["可见命令名", port.commandRedacted],
                        ["归属证据", port.evidence],
                      ].map(([label, value]) => (
                        <DetailListItem key={label}>
                          <DetailListLabel>{label}</DetailListLabel>
                          <DetailListValue className="break-all">
                            {value}
                          </DetailListValue>
                        </DetailListItem>
                      ))}
                    </DetailList>
                    <p className="text-label text-fg-muted">
                      没有可确认的 Agent / Session 启动来源。
                    </p>
                  </div>
                </AccordionContent>
              </AccordionItem>
            ))}
          </div>
        </Accordion>
      ) : (
        <p className="text-body text-fg-muted">
          {ports.state === "ready"
            ? "当前未发现项目监听端口。"
            : "尚无法确认项目监听端口，数量保持未知。"}
        </p>
      )}
    </>
  );
}
