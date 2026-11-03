import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { money, summaryMoney } from "@/shared/lib/money";
import type { AssociatedCosts, CostTotals } from "@/shared/types/cost";

export function CostTotalsView({
  cost,
  label = "费用统计",
  compact = false,
}: {
  cost?: CostTotals | null;
  label?: string;
  compact?: boolean;
}) {
  const format = (amount: number, currency: string) =>
    compact ? summaryMoney(amount, currency) : money(amount, currency, 9);
  return (
    <dl aria-label={label} className="grid gap-4 sm:grid-cols-3">
      {(
        [
          [
            "实际费用",
            cost?.actual.map((m) => format(m.amount, m.currency)).join(" / ") ||
              "未知",
          ],
          [
            "估算费用",
            cost?.estimated
              .map((m) => format(m.amount, m.currency))
              .join(" / ") || "未知",
          ],
          ["计价未知", cost ? `${cost.unknownCalls} 次` : "未知"],
        ] as const
      ).map(([name, value]) => (
        <div key={name} className="min-w-0">
          <dt className="text-label text-fg-muted">{name}</dt>
          <dd
            className="mt-1 break-words text-title font-medium tabular-nums"
            title={
              name === "实际费用"
                ? cost?.actual
                    .map((m) => money(m.amount, m.currency, 9))
                    .join(" / ")
                : name === "估算费用"
                  ? cost?.estimated
                      .map((m) => money(m.amount, m.currency, 9))
                      .join(" / ")
                  : undefined
            }
          >
            {value}
          </dd>
        </div>
      ))}
    </dl>
  );
}
export function AssociatedCostsView({
  value,
  label = "关联费用",
}: {
  value?: AssociatedCosts | null;
  label?: string;
}) {
  return (
    <section aria-label={label} className="space-y-3">
      <h3 className="text-title font-medium">{label}</h3>
      <CostTotalsView
        compact
        cost={value?.gatewayAttempts ? value.cost : undefined}
      />
      <p className="text-label text-fg-muted">
        {!value
          ? "费用尚未读取"
          : value.gatewayAttempts === 0
            ? value.nativeResponses > 0
              ? "未关联到网关调用，费用未知"
              : "暂无已关联调用"
            : `已关联 ${value.gatewayAttempts} 次网关尝试`}
        {value && value.unlinkedNativeResponses > 0 ? " · 原生覆盖未确认" : ""}
        {value?.sourceState === "partial" ? " · 采集不完整" : ""}
      </p>
      <Accordion>
        <AccordionItem value="scope">
          <AccordionTrigger>费用统计依据</AccordionTrigger>
          <AccordionContent>
            <div className="space-y-2 text-label text-fg-muted">
              <p>
                金额仅累计已确认关联的网关尝试，每次计一份；实际与估算分别汇总，不与原生用量或订阅额度相加。缺少计价依据的金额保留未知。
              </p>
              {value && (
                <>
                  <p>
                    完整金额：实际{" "}
                    {value.cost.actual
                      .map((m) => money(m.amount, m.currency, 9))
                      .join(" / ") || "未知"}
                    ；估算{" "}
                    {value.cost.estimated
                      .map((m) => money(m.amount, m.currency, 9))
                      .join(" / ") || "未知"}
                    。
                  </p>
                  <p>
                    {value.responseLinkedAttempts} 次通过响应 ID 关联；
                    {value.sessionLinkedAttempts} 次通过会话 ID 关联。会话 ID
                    不代表逐响应一一对应。
                  </p>
                  <p>
                    {value.nativeResponses} 个原生响应，
                    {value.matchedNativeResponses} 个已逐条核对；
                    {value.unlinkedNativeResponses}{" "}
                    个未核对，费用覆盖未知。这些响应不再增加费用或计价未知次数。
                  </p>
                  {value.conflictingNativeResponses > 0 && (
                    <p>
                      {value.conflictingNativeResponses}{" "}
                      个响应存在关联或用量冲突，未套用网关费用。
                    </p>
                  )}
                </>
              )}
            </div>
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </section>
  );
}
