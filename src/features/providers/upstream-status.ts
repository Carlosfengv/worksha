import type { UpstreamRecoveryState } from "./types";

export const verificationLabels = {
  verified: "验证通过",
  expired: "验证已过期",
  failed: "验证失败",
  outdated: "需重新验证",
  unknown: "验证状态未知",
  unverified: "尚未验证",
} as const;

export const cooldownLabels: Record<string, string> = {
  upstream_rate_limit: "上游限流",
  upstream_credential_rejected: "上游拒绝凭据",
  upstream_model_access_denied: "上游拒绝模型权限",
  upstream_failure: "上游临时故障",
  stream_interrupted_after_commit: "流式响应中断",
};
export const recoveryLabels: Record<UpstreamRecoveryState, string> = {
  waitingForProbe: "待恢复探测",
  probing: "账号恢复探测中",
};
