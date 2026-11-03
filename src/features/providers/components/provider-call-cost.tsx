import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import type { ProviderUsage } from "../types";
import { costReason, providerMoney } from "../usage-display";
import { date } from "@/features/sessions/display";

export function ProviderCallCost({
  call,
}: {
  call: Pick<ProviderUsage["calls"][number], "cost" | "model" | "time">;
}) {
  const cost = call.cost;
  const label =
    cost.kind === "actual"
      ? "实际费用"
      : cost.kind === "estimated"
        ? "估算费用"
        : "费用未知";
  const amount =
    cost.amount != null && cost.currency
      ? providerMoney(cost.amount, cost.currency, 9)
      : "未知";
  return (
    <Dialog>
      <DialogTrigger
        render={
          <Button
            variant="link"
            size="sm"
            aria-label={
              cost.kind === "unknown"
                ? "费用未知，查看原因"
                : `${label} ${amount}，查看明细`
            }
          />
        }
      >
        <span>
          {amount}
          {cost.kind !== "unknown" && (
            <span className="block text-label text-fg-muted">{label}</span>
          )}
        </span>
      </DialogTrigger>
      <DialogContent size="lg">
        <DialogHeader>
          <DialogTitle>{label}</DialogTitle>
          <DialogDescription>
            {call.model ?? "模型未记录"} · {date(call.time)}
          </DialogDescription>
        </DialogHeader>
        <div className="max-h-96 space-y-4 overflow-y-auto">
          {cost.kind === "unknown" ? (
            <p>{costReason(cost.reason)}</p>
          ) : (
            <>
              <p className="text-title font-medium">
                {providerMoney(cost.amount!, cost.currency!, 9)}
              </p>
              {cost.kind === "actual" ? (
                <p className="text-label text-fg-muted">
                  服务商已报告本次账户扣费。
                </p>
              ) : (
                <>
                  <Table aria-label="本次调用费用明细">
                    <TableHeader>
                      <TableRow>
                        <TableHead>项目</TableHead>
                        <TableHead>Token</TableHead>
                        <TableHead>单价 / 百万</TableHead>
                        <TableHead>费用</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {cost.components.map((c) => (
                        <TableRow key={c.kind}>
                          <TableCell>
                            {{
                              input: "未缓存输入",
                              cached_input: "缓存命中",
                              output: "输出（含推理）",
                            }[c.kind] ?? c.kind}
                          </TableCell>
                          <TableCell>
                            {c.tokens.toLocaleString("zh-CN")}
                          </TableCell>
                          <TableCell>
                            {providerMoney(c.perMillion, cost.currency!)}
                          </TableCell>
                          <TableCell>
                            {providerMoney(c.amount, cost.currency!, 9)}
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                  {cost.price && (
                    <div className="space-y-1 text-label text-fg-muted">
                      <p>
                        {cost.price.model} ·{" "}
                        {cost.price.period === "peak" ? "高峰" : "非高峰"}
                      </p>
                      <p>
                        价格核实于 {cost.price.checkedAt} · {cost.price.version}
                      </p>
                      <p>
                        估算覆盖期：{date(cost.price.validFrom)} 至{" "}
                        {date(cost.price.validUntil)}（不含）
                      </p>
                      <p>按真实 Token 估算，仅含模型 Token 费用。</p>
                    </div>
                  )}
                </>
              )}
              {cost.source && (
                <Button variant="link" size="sm" asChild>
                  <a href={cost.source} target="_blank" rel="noreferrer">
                    计费来源
                  </a>
                </Button>
              )}
            </>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
