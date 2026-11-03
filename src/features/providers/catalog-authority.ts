import type { SourceState } from "../../../contracts/domain/provider-source/types";

// A remembered implementation is not authority. Include the exact current
// grant and source token even when endpoint/protocol/credential are unchanged.
export function catalogAuthority(state: SourceState): string {
  if (!state.authorized || !["builtin", "external"].includes(state.kind))
    throw Error(
      state.kind === "pending"
        ? "目录来源仍待原操作核查，请先处理恢复记录"
        : "目录来源尚未授权或已变化，请在连接配置中核对并授权当前已保存修订",
    );
  return JSON.stringify([
    state.providerId,
    state.expectedRevision,
    state.sourceRevision ?? null,
    state.kind,
    state.choice?.pluginId ?? null,
    state.choice?.digest ?? null,
    state.selection?.bindingId ?? null,
    state.policy
      ? [
          state.policy.schemaVersion,
          state.policy.baseProtocol,
          state.policy.path,
        ]
      : null,
  ]);
}
