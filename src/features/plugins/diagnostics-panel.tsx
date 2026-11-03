import type { PluginStatus } from "./types";
import { isExtension } from "./capabilities";
import { formatDateTime } from "@/shared/lib/date-time";
import {
  capabilityDetails,
  operationNames,
  snapshotNames,
  type PluginDiagnostics,
} from "./diagnostics";

const errorNames: Record<string, string> = {
  deadline_exceeded: "运行超时",
  resource_limit: "资源或并发限制",
  worker_crashed: "运行进程异常",
  permission_denied: "所需资源未授权",
  grant_revoked: "授权或版本已变化",
  stale_snapshot: "快照已失效",
  plugin_disabled: "插件未启用",
  signature_invalid: "签名或摘要校验失败",
  invalid_input: "输入或插件结果不符合约定",
  result_pending: "操作结果待核查",
  interrupted: "操作未完成，结果未知",
};
const time = (value: string | null) =>
  value ? formatDateTime(value, "full") : "尚无记录";
const count = (value: string) => BigInt(value).toLocaleString("zh-CN");
export function capabilityNames(status: PluginStatus) {
  return status.manifest.capabilities
    .map((key) =>
      Object.hasOwn(capabilityDetails, key)
        ? capabilityDetails[key].name
        : "未识别能力",
    )
    .join("、");
}
export function PluginDiagnosticsPanel({
  status,
  diagnostics,
  unavailable = false,
}: {
  status: PluginStatus;
  diagnostics?: PluginDiagnostics | null;
  unavailable?: boolean;
}) {
  const row = diagnostics?.items.find(
    (item) =>
      item.pluginId === status.manifest.id && item.digest === status.digest,
  );
  const activity = row?.activity;
  const grants = [
    status.manifest.capabilities.includes("session.source@1") &&
      `历史来源：${status.sourceAuthorized ? "已授权" : "待授权"}`,
    status.manifest.capabilities.includes("account.quota-source@1") &&
      `额度连接：${status.connectionAuthorized ? "已授权" : "待授权"}`,
    status.manifest.capabilities.includes("report.view@1") &&
      `汇总报表：${status.reportAuthorized ? "已授权" : "待授权"}`,
    status.manifest.capabilities.includes("provider.models@1") &&
      "模型目录：具体连接范围与授权状态见模型目录接入区",
  ].filter(Boolean);
  return (
    <section className="space-y-3" aria-label="插件能力与运行诊断">
      <h4 className="text-title font-medium">能力与运行诊断</h4>
      <p className="text-label text-fg-muted">
        {status.enabled ? "此版本已启用" : "此版本未启用"} ·{" "}
        {status.manifest.runtime.kind === "declarative"
          ? "声明式扩展"
          : "隔离 Wasm 运行"}
        。
        {status.manifest.capabilities.includes("integration.connector@1")
          ? "MCP 程序和连接的实际状态见下方；stdio 程序以当前用户权限运行。"
          : "启用表示初始化通过；资源读取或连接成功以实际操作结果为准。"}
      </p>
      <ul className="space-y-2 text-label">
        {status.manifest.capabilities.map((key) => (
          <li key={key}>
            <span className="font-medium">
              {Object.hasOwn(capabilityDetails, key)
                ? capabilityDetails[key].name
                : "未识别能力"}
            </span>
            <span className="text-fg-muted">
              ：
              {(Object.hasOwn(capabilityDetails, key)
                ? capabilityDetails[key].scope
                : null) ?? "当前宿主未确认此能力的支持范围。"}
            </span>
          </li>
        ))}
      </ul>
      <p className="text-label text-fg-muted">
        {grants.length
          ? grants.join("；")
          : isExtension(status.manifest)
            ? "资源授权与连接状态见下方专用操作区。"
            : "此版本没有声明来源、额度或报表读取权限。"}
      </p>
      {unavailable || !row ? (
        <p role="status" className="text-label text-fg-muted">
          运行诊断暂不可用；启用和授权状态以当前管理记录为准。
        </p>
      ) : (
        <>
          <dl className="grid gap-3 text-label sm:grid-cols-2">
            <div>
              <dt className="text-fg-muted">数据读取</dt>
              <dd>{snapshotNames[row.snapshot.state]}</dd>
            </div>
            <div>
              <dt className="text-fg-muted">最近完整读取</dt>
              <dd>{time(row.snapshot.observedAt)}</dd>
            </div>
            {row.snapshot.records !== null && (
              <div>
                <dt className="text-fg-muted">完整快照记录数</dt>
                <dd>{count(row.snapshot.records)}</dd>
              </div>
            )}
          </dl>
          <p className="text-label text-fg-muted">
            诊断范围：本次服务启动以来（{time(diagnostics?.since ?? null)}
            ）。管理操作次数独立于模型调用和 Token；重启后重新计数。
          </p>
          {diagnostics && diagnostics.dropped !== "0" && (
            <p role="status" className="text-label text-warning-fg">
              部分操作诊断未记录（{count(diagnostics.dropped)}{" "}
              次），下方计数可能不完整。
            </p>
          )}
          {activity ? (
            <dl className="grid gap-3 text-label sm:grid-cols-2 lg:grid-cols-3">
              <div>
                <dt className="text-fg-muted">已观测管理操作</dt>
                <dd>
                  {count(activity.attempts)} 次；进行中{" "}
                  {count(activity.inFlight)} 次
                </dd>
              </div>
              <div>
                <dt className="text-fg-muted">操作结果</dt>
                <dd>
                  成功 {count(activity.succeeded)}；失败{" "}
                  {count(activity.failed)}；未完成 {count(activity.interrupted)}
                </dd>
              </div>
              <div>
                <dt className="text-fg-muted">失败分类</dt>
                <dd>
                  超时 {count(activity.timeouts)}；资源限制{" "}
                  {count(activity.resourceLimits)}；进程异常{" "}
                  {count(activity.workerCrashes)}
                </dd>
              </div>
              <div>
                <dt className="text-fg-muted">最近完成操作</dt>
                <dd>
                  {activity.lastOperation
                    ? operationNames[activity.lastOperation]
                    : "尚无记录"}{" "}
                  · {time(activity.lastFinishedAt)}
                </dd>
              </div>
              <div>
                <dt className="text-fg-muted">最近成功操作</dt>
                <dd>{time(activity.lastSuccessAt)}</dd>
              </div>
              <div>
                <dt className="text-fg-muted">最近操作耗时</dt>
                <dd>
                  {activity.lastDurationMs === null
                    ? "未知"
                    : `${count(activity.lastDurationMs)} ms`}
                </dd>
              </div>
              {activity.lastErrorCode && (
                <div>
                  <dt className="text-fg-muted">最近操作问题</dt>
                  <dd>
                    {(Object.hasOwn(errorNames, activity.lastErrorCode)
                      ? errorNames[activity.lastErrorCode]
                      : null) ?? "操作未完成，请检查具体能力状态。"}
                  </dd>
                </div>
              )}
            </dl>
          ) : (
            <p className="text-label text-fg-muted">
              本次服务启动以来尚无此版本的操作诊断；历史运行情况未知。
            </p>
          )}
        </>
      )}
    </section>
  );
}
