import { Badge } from "@/components/ui/badge";
import type { ProviderStatus } from "@/features/providers/types";
import { dynamicBindingFor } from "@/features/providers/types";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { providerStatusResource } from "../queries";
import type { ConfigAgentId } from "@/shared/types/agent";

export function activationLabel(status: ProviderStatus, agentId: string) {
  const config = status.configs.find((c) => c.agentId === agentId);
  const dynamic = config ? dynamicBindingFor(status, config) : undefined;
  if (dynamic?.lastObserved) return null;
  const latest = status.transactions.find(
    (t) =>
      t.agentId === agentId &&
      ["pending", "applying", "applied"].includes(t.state),
  );
  if (!latest) return null;
  if (latest.state === "pending") return "待重启后生效";
  if (latest.state === "applying") return "正在写入配置";
  if (latest.activation?.state === "launch_requested")
    return "已请求启动 · 待调用确认";
  if (latest.activation?.state === "awaiting_restart")
    return "配置已写入 · 待重启后生效";
  return null;
}

export function ProviderActivationStatus({
  agentId,
}: {
  agentId: ConfigAgentId;
}) {
  const status = useSharedRead(providerStatusResource);
  const label = status.data ? activationLabel(status.data, agentId) : null;
  if (status.error) return <Badge status="warning">配置生效状态未知</Badge>;
  return label ? <Badge status="warning">{label}</Badge> : null;
}
