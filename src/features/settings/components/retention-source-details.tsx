import { Alert, AlertTitle, AlertDescription } from "@/components/ui/alert";
import { formatDateTime } from "@/shared/lib/date-time";
import {
  DetailList,
  DetailListItem,
  DetailListLabel,
  DetailListValue,
} from "@/components/ui/detail-list";
import type { GatewayImpact } from "../retention-result";
import type { ActivityImpact } from "../retention-content-inventory";
import {
  retentionSourceIssues,
  type SourceReport,
  type ToolImpact,
} from "../retention-inventory";

export const retentionDate = (value: string) => formatDateTime(value, "full");
export function UnverifiedRetentionSource({
  reason,
}: {
  reason: Extract<SourceReport<unknown>, { status: "unverified" }>["reason"];
}) {
  return (
    <Alert status="warning">
      <AlertTitle>此来源尚未完成核查</AlertTitle>
      <AlertDescription>{retentionSourceIssues[reason]}</AlertDescription>
    </Alert>
  );
}
export function GatewayRetentionDetails({
  source,
}: {
  source: Extract<SourceReport<GatewayImpact>, { status: "checked" }>;
}) {
  const g = source.value;
  const counts: [string, number][] = [
    ["已入库尝试", g.attempts],
    ["可分组的逻辑请求", g.logicalRequests],
    ["超过所选期限", g.expired],
    ["期限内", g.withinWindow],
    ["活跃请求保护", g.protected.active],
    ["结果不明保护", g.protected.unknownOutcome],
    ["时间异常保护", g.protected.unknownTime],
    ["来源结构待核查", g.protected.unsupportedSource],
    ["无法分组的原始尝试", g.unreadableAttempts],
  ];
  return (
    <>
      <p className="text-body text-fg-muted">
        本来源读取：{retentionDate(source.readStartedAt)} 至{" "}
        {retentionDate(source.readFinishedAt)}
      </p>
      <DetailList>
        {counts.map(([label, value]) => (
          <DetailListItem key={label}>
            <DetailListLabel>{label}</DetailListLabel>
            <DetailListValue>{value}</DetailListValue>
          </DetailListItem>
        ))}
      </DetailList>
      {g.attempts === 0 && (
        <p className="text-body text-fg-muted">
          已确认本次检查的网关来源为空。
        </p>
      )}
    </>
  );
}
export function ToolRetentionDetails({
  source,
}: {
  source: Extract<SourceReport<ToolImpact>, { status: "checked" }>;
}) {
  const t = source.value;
  const counts: [string, number][] = [
    ["当前保存工具记录", t.records],
    ["超过期限的工具记录", t.expired],
    ["期限内工具记录", t.withinWindow],
    ["活跃工具保护", t.protected.active],
    ["工具结果不明保护", t.protected.unknownOutcome],
    ["工具时间异常保护", t.protected.unknownTime],
    ["工具来源结构待核查", t.protected.unsupportedSource],
    ["无法读取的工具记录", t.unreadableRecords],
  ];
  return (
    <>
      <p className="text-body text-fg-muted">
        本来源读取：{retentionDate(source.readStartedAt)} 至{" "}
        {retentionDate(source.readFinishedAt)}
      </p>
      <DetailList>
        {counts.map(([label, value]) => (
          <DetailListItem key={label}>
            <DetailListLabel>{label}</DetailListLabel>
            <DetailListValue>{value}</DetailListValue>
          </DetailListItem>
        ))}
      </DetailList>
      {t.records === 0 && (
        <p className="text-body text-fg-muted">
          已确认本次检查的工具保存集合为空。
        </p>
      )}
      <p className="text-body text-fg-muted">
        {t.capacityLimit === 10_000
          ? "当前保存容量上限为 10000 条。"
          : "当前容量策略尚未核对。"}{" "}
        仅统计当前保存集合，不能确认曾经的完整历史；已按容量移除的记录不在本次数量中。
      </p>
      <p className="text-body text-fg-muted">
        按工具记录时间判断期限。超时、取消和传输失败仍受保护；不据此推定远端操作已经完成。
      </p>
    </>
  );
}

export function ActivityRetentionDetails({
  source,
}: {
  source: Extract<SourceReport<ActivityImpact>, { status: "checked" }>;
}) {
  const a = source.value;
  const counts: [string, number][] = [
    ["当前保存摘要", a.records],
    ["摘要事件早于期限", a.olderThanCutoff],
    ["摘要事件在期限内", a.withinWindow],
    ["摘要时间待核查", a.unknownTime],
  ];
  return (
    <>
      <p className="text-body text-fg-muted">
        本来源读取：{retentionDate(source.readStartedAt)} 至{" "}
        {retentionDate(source.readFinishedAt)}
      </p>
      <DetailList>
        {counts.map(([label, value]) => (
          <DetailListItem key={label}>
            <DetailListLabel>{label}</DetailListLabel>
            <DetailListValue>{value}</DetailListValue>
          </DetailListItem>
        ))}
      </DetailList>
      {a.records === 0 && (
        <p className="text-body text-fg-muted">
          已确认本次检查的活动摘要缓存为空。
        </p>
      )}
      <p className="text-body text-fg-muted">
        仅统计本产品保存的短文本副本，按摘要事件时间分组；不显示摘要内容，也不表示这些记录已获准删除。
      </p>
    </>
  );
}
