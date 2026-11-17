import { useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  InlineNotice,
  InlineNoticeContent,
} from "@/components/ui/inline-notice";
import { number, sessionLink } from "@/features/sessions/display";
import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { isDesktopRuntime } from "@/shared/lib/desktop-command";
import { createReadResource } from "@/shared/lib/read-resource";
import { observationApi, type LoopAlert, type LoopOverride } from "../api";

const resource = createReadResource(observationApi.loopGuards, {
  interval: 5000,
});
const reasonTitle: Record<LoopAlert["reason"], string> = {
  toolErrorStreak: "连续工具报错",
  actionPingPong: "重复调用相同工具与参数",
  unattendedBurn: "续调步数或用量达到阈值",
  dedupCapacity: "工具去重容量已满",
};

function detail(alert: LoopAlert): string {
  switch (alert.reason) {
    case "toolErrorStreak":
      return `连续 ${alert.consecutiveToolErrors} 步报错`;
    case "actionPingPong":
      return `相同工具与参数调用 ${alert.repeatedActions} 次`;
    case "dedupCapacity":
      return "请发送新用户指令或开启新会话。";
    case "unattendedBurn": {
      const rows = [
        alert.usage.committed,
        alert.usage.reserved,
        alert.usage.frozen,
      ];
      const tokens = rows.reduce((sum, row) => sum + (row.tokens ?? 0), 0);
      const usd = rows.reduce((sum, row) => sum + (row.usdMicros ?? 0), 0);
      const cost =
        usd > 0 || alert.usage.usdUnknown === 0
          ? [`已计量 USD ${(usd / 1_000_000).toFixed(2)}`]
          : [];
      if (alert.usage.usdUnknown > 0) cost.push("部分费用未知");
      return [
        `${alert.continuationSteps} 步续调`,
        `${number(tokens)} Tokens`,
        ...cost,
      ].join(" · ");
    }
  }
}

export function LoopGuardAttention() {
  const guards = useSharedRead(resource, isDesktopRuntime());
  const { agentNames } = useAgentDirectory();
  const [pending, setPending] = useState<LoopOverride | null>(null);
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState("");
  const [error, setError] = useState("");
  const alerts = guards.data?.alerts ?? [];
  const storeId = guards.data?.storeId;
  if (!alerts.length && !pending && !feedback && !guards.error) return null;

  async function release() {
    if (!pending || busy) return;
    setBusy(true);
    setError("");
    try {
      await observationApi.overrideLoopGuard(pending);
      setPending(null);
      setFeedback("已放行本轮。请在原客户端重试。");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      await guards.invalidate();
      setBusy(false);
    }
  }

  return (
    <section aria-label="本机熔断与预警" className="space-y-2">
      {feedback && (
        <p role="status" className="text-label">
          {feedback}
        </p>
      )}
      {guards.error && (
        <p role="alert" className="text-label">
          本机熔断状态暂不可读取。{guards.error}
        </p>
      )}
      <ul className="space-y-2">
        {alerts.map((alert) => (
          <li key={JSON.stringify([alert.bindingId, alert.sessionId])}>
            <InlineNotice className="w-full gap-x-2 px-2 text-label">
              <Badge
                variant="plain"
                size="sm"
                status={alert.status === "tripped" ? "danger" : "warning"}
              >
                {alert.status === "tripped" ? "熔断" : "预警"}
              </Badge>
              <InlineNoticeContent className="flex flex-wrap items-center gap-x-2 gap-y-1 wrap-anywhere">
                <span className="font-medium">{reasonTitle[alert.reason]}</span>
                <span className="text-fg-muted">
                  {alert.bindingName ??
                    agentNames[alert.agentId] ??
                    alert.agentId}{" "}
                  · {detail(alert)}
                </span>
                <a
                  href={`#${sessionLink(alert.sessionId, { view: "requests" })}`}
                  className="rounded-sm underline decoration-border underline-offset-2 hover:decoration-current focus-visible:ring-1 focus-visible:ring-focus-ring"
                >
                  查看会话详情
                </a>
                {alert.status === "tripped" &&
                  alert.reason !== "dedupCapacity" &&
                  storeId &&
                  !guards.error && (
                    <Button
                      size="sm"
                      variant="secondary"
                      disabled={busy}
                      onClick={() => {
                        setError("");
                        setFeedback("");
                        setPending({
                          storeId,
                          operationId: crypto.randomUUID(),
                          bindingId: alert.bindingId,
                          sessionId: alert.sessionId,
                          expectedTurnDigest: alert.turnDigest,
                          expectedOverrideEpoch: alert.overrideEpoch,
                        });
                      }}
                    >
                      放行本轮
                    </Button>
                  )}
              </InlineNoticeContent>
            </InlineNotice>
          </li>
        ))}
      </ul>
      <Dialog
        open={pending !== null}
        onOpenChange={(open) => {
          if (!open && !busy) setPending(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>放行本轮</DialogTitle>
            <DialogDescription>
              允许客户端重试本轮。新增 20 步续调、250,000 Tokens 或 USD 1.50
              时将再次熔断。连续报错和重复工具调用仍受保护。
            </DialogDescription>
          </DialogHeader>
          {error && <p role="alert">{error}</p>}
          <DialogFooter>
            <Button
              variant="secondary"
              disabled={busy}
              onClick={() => setPending(null)}
            >
              取消
            </Button>
            <Button disabled={busy} onClick={() => void release()}>
              {busy ? "正在放行" : "确认放行"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
