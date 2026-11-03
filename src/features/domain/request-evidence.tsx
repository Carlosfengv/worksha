import { Button } from "@/components/ui/button";
import type {
  RequestBilling,
  RequestEvidenceDetail,
  RequestEvidenceSummary,
  NativeMatchState,
} from "../../../contracts/domain/facts/types";

const count = (v: string | null | undefined) =>
  v == null ? "未知" : BigInt(v).toLocaleString("zh-CN");
const states: Record<NativeMatchState, string> = {
  "exact-response-id": "响应 ID 精确关联",
  unlinked: "尚未关联",
  ambiguous: "关联有歧义",
  "identity-conflict": "身份冲突",
  unknown: "关联证据未知",
};
function Billing({ value }: { value: RequestBilling | null | undefined }) {
  if (!value) return <p>未提供宿主计费证据。</p>;
  const amounts = (rows: RequestBilling["actual"]) =>
    rows
      .map((v) => `${v.amount} ${v.currency}（${count(v.parts)} 项）`)
      .join("；") || "未记录";
  return (
    <div className="space-y-1 break-words">
      <p>实付：{amounts(value.actual)}</p>
      <p>估算：{amounts(value.estimated)}</p>
      <p>
        费用未知项：{count(value.unknownParts)}。各币种分别保留，不作汇率换算。
      </p>
    </div>
  );
}
export function RequestEvidenceTotals({
  value,
}: {
  value: RequestEvidenceSummary | null | undefined;
}) {
  if (!value) return null;
  const match = value.matching;
  return (
    <div className="space-y-1">
      {match.kind === "native-responses" ? (
        <p>
          精确关联 {count(match.exact)} 个响应；未关联 {count(match.unlinked)}
          ，歧义 {count(match.ambiguous)}，身份冲突{" "}
          {count(match.identityConflicts)}，未知 {count(match.unknown)}
          ，用量冲突 {count(match.usageConflicts)}。
        </p>
      ) : match.kind === "gateway-requests" ? (
        <p>
          有精确关联的逻辑请求 {count(match.withExactLinks)} 个；尚未确认关联{" "}
          {count(match.withoutConfirmedLinks)} 个；关联原生响应{" "}
          {count(match.matchedResponses)} 个，用量冲突{" "}
          {count(match.usageConflictLinks)} 个。没有确认关联不等于没有重叠。
        </p>
      ) : (
        <p>来源报告的 {count(match.unverifiedRecords)} 条记录关联未经核实。</p>
      )}
      {value.overflowFields.length ? (
        <p>部分用量合计超出精确计数上限，合计保持未知，原记录数值仍可查看。</p>
      ) : null}
      <p>底层总用量缺失项：{count(value.usageMissingParts)}。</p>
      <Billing value={value.billing} />
    </div>
  );
}
export function RequestEvidenceDetails({
  value,
  onSession,
}: {
  value: RequestEvidenceDetail | null | undefined;
  onSession: (id: string, key: string) => void;
}) {
  if (!value) return null;
  const match = value.matching;
  return (
    <section aria-label="冻结关联与费用证据" className="space-y-3">
      {match.kind === "native-response" ? (
        <>
          <p>
            {states[match.state]}
            {match.usageConflict ? " · 用量有冲突，保留双方原值" : ""}。
          </p>
          {match.responseId ? (
            <p className="break-all">原生响应 ID：{match.responseId}</p>
          ) : null}
          {match.state === "exact-response-id" && match.logicalRequestId ? (
            <Button asChild variant="tertiary" size="sm">
              <a
                href={`#/requests/${encodeURIComponent(match.logicalRequestId)}`}
              >
                查看精确关联的网关请求
              </a>
            </Button>
          ) : null}
        </>
      ) : match.kind === "gateway-request" ? (
        <>
          <p>精确关联 {count(match.matchedResponses)} 个原生响应。</p>
          {match.links.map((link) => {
            const key = JSON.stringify([link.eventId, link.attemptId]);
            return (
              <div key={key} className="space-y-1">
                <p className="break-all">
                  {link.responseId}
                  {link.usageConflict ? " · 用量冲突" : ""}
                </p>
                <Button
                  variant="tertiary"
                  size="sm"
                  data-request-navigation={key}
                  onClick={() => onSession(link.sessionId, key)}
                >
                  查看关联会话
                </Button>
              </div>
            );
          })}
          {match.linksLimited ? (
            <p>
              此处显示前 40 个关联，完整数量保留；其余关联可进入原请求详情查看。
            </p>
          ) : null}
        </>
      ) : (
        <p>来源报告不建立宿主精确关联，也不参与跨来源去重。</p>
      )}
      <Billing value={value.billing} />
      {value.timingNote ? (
        <p className="break-words text-fg-muted">
          计时说明：{value.timingNote}
        </p>
      ) : null}
      {value.overflowFields.length ? (
        <p>
          本条用量合计超出精确计数上限，合计保持未知，原始尝试数值仍可查看。
        </p>
      ) : null}
      <p className="text-fg-subtle">
        以上为本次列表的冻结证据；原请求详情仍按原有方式读取。
      </p>
    </section>
  );
}
