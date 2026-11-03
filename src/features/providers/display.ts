import type { Provider, ProviderVerification } from "./types";
export const protocolName = (p: Provider["protocol"]) =>
  p === "messages"
    ? "Messages"
    : p === "responses"
      ? "Responses"
      : "Chat Completions";
export const verificationName = (v?: ProviderVerification) =>
  v
    ? ({
        verified: "连接已验证",
        failed: "验证失败",
        expired: "验证已过期",
        outdated: "配置已变更 · 待验证",
      }[v.state] ?? "未验证")
    : "未验证";
export const message = (reason: unknown) =>
  reason instanceof Error ? reason.message : String(reason);

export { configAgentName as agentName } from "@/shared/types/agent";
