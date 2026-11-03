import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import { CostTotalsView } from "@/shared/ui/associated-costs";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { agentsApi } from "@/features/agents/api";
import type { AgentUsage, AgentUsageQuery } from "@/features/agents/types";
import { usageNumber } from "@/features/agents/usage-display";
import { date, number, sessionLink } from "@/features/sessions/display";
import { usePollingResource } from "@/shared/hooks/use-polling-resource";
import { type AgentProductId } from "@/shared/types/agent";
import type { Usage } from "@/shared/types/usage";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { useCallback, useState } from "react";
import { ModelUsageTable } from "./model-usage-table";

const usageFields: [keyof Usage, string][] = [
  ["inputTokens", "输入 Token"],
  ["outputTokens", "输出 Token"],
  ["cachedInputTokens", "缓存读取 · 输入子集"],
  ["cacheWriteInputTokens", "缓存写入 · 输入子集"],
  ["reasoningOutputTokens", "推理 Token · 输出子集"],
  ["totalTokens", "总 Token"],
];
type UsageItem = AgentUsage["items"][number];

export function AgentModelUsage({
  agentId,
  nativeEnabled,
}: {
  agentId: AgentProductId;
  nativeEnabled: boolean;
}) {
  const { agentFamily, isSharedSource } = useAgentDirectory();
  const [source, setSource] = useState<AgentUsageQuery["source"]>(
    nativeEnabled || agentId === "antigravity" ? "native" : "gateway",
  );
  const [days, setDays] = useState("all");
  const [attribution, setAttribution] = useState("current");
  const queryAgent =
    attribution === "unknown" ? `${agentFamily(agentId)}-unknown` : agentId;
  const [provider, setProvider] = useState("all");
  const [model, setModel] = useState("");
  const [pages, setPages] = useState<string[]>([]);
  const [selected, setSelected] = useState<{
    row: UsageItem;
    asOf: string;
  } | null>(null);
  const cursor = pages.at(-1);
  const loader = useCallback(
    () =>
      agentsApi.agentUsage({
        agent: queryAgent,
        source,
        days: days === "all" ? undefined : Number(days),
        provider: provider === "all" ? undefined : provider,
        model: model.trim() || undefined,
        cursor,
      }),
    [queryAgent, source, days, provider, model, cursor],
  );
  const resource = usePollingResource(
    `agent-usage:${queryAgent}:${source}:${days}:${provider}:${model}:${cursor}`,
    loader,
  );
  const data = resource.data;
  const countLabel = source === "native" ? "已识别模型响应" : "网关请求尝试";
  const stale =
    Boolean(resource.error) ||
    Boolean(data && Date.now() - Date.parse(data.asOf) > 30000);
  const disconnected =
    data?.sourceState === "disabled" || data?.sourceState === "pending";
  const changeFilter = () => {
    setPages([]);
    setSelected(null);
  };
  return (
    <section className="min-w-0 space-y-4" aria-label="Agent 模型用量">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-title font-medium">模型用量</h2>
        </div>
        <Button
          variant="tertiary"
          size="sm"
          loading={resource.loading}
          onClick={resource.reload}
        >
          刷新模型用量
        </Button>
      </div>
      <div className="flex flex-wrap items-end gap-3">
        {isSharedSource(agentFamily(agentId) ?? agentId) && (
          <Select
            value={attribution}
            onValueChange={(value) => {
              setAttribution(value);
              setProvider("all");
              changeFilter();
            }}
          >
            <SelectTrigger aria-label="用量归属" />
            <SelectContent>
              <SelectItem value="current">当前 Agent</SelectItem>
              <SelectItem value="unknown">同家客户端来源未知</SelectItem>
            </SelectContent>
          </Select>
        )}
        <Select
          value={source}
          onValueChange={(value) => {
            setSource(value as AgentUsageQuery["source"]);
            setProvider("all");
            changeFilter();
          }}
        >
          <SelectTrigger aria-label="模型用量来源" />
          <SelectContent>
            <SelectItem value="native">原生记录 · 官方 / 第三方</SelectItem>
            {agentId !== "antigravity" && (
              <SelectItem value="gateway">本地网关 · 上游服务</SelectItem>
            )}
          </SelectContent>
        </Select>
        <Select
          value={days}
          onValueChange={(value) => {
            setDays(value);
            setProvider("all");
            changeFilter();
          }}
        >
          <SelectTrigger aria-label="模型调用日期范围" />
          <SelectContent>
            <SelectItem value="all">全部调用日期</SelectItem>
            <SelectItem value="1">最近 24 小时</SelectItem>
            <SelectItem value="7">最近 7 天</SelectItem>
            <SelectItem value="30">最近 30 天</SelectItem>
          </SelectContent>
        </Select>
        <Select
          value={provider}
          onValueChange={(value) => {
            setProvider(value);
            changeFilter();
          }}
        >
          <SelectTrigger aria-label="模型用量 Provider" />
          <SelectContent>
            <SelectItem value="all">全部 Providers</SelectItem>
            {data?.providers.map((item) => (
              <SelectItem key={item.id} value={item.id}>
                {item.name ?? "Provider 未记录"}
                {item.endpoint ? ` · ${item.endpoint}` : ""}
                {item.routeRevision !== null
                  ? ` · 路由版本 ${item.routeRevision}`
                  : ""}
              </SelectItem>
            ))}
            {provider !== "all" &&
              !data?.providers.some((item) => item.id === provider) && (
                <SelectItem value={provider}>
                  已选 Provider · 等待读取
                </SelectItem>
              )}
          </SelectContent>
        </Select>
        <div className="min-w-0 flex-1">
          <Input
            aria-label="搜索模型用量"
            placeholder="搜索模型"
            value={model}
            maxLength={200}
            onChange={(event) => {
              setModel(event.target.value);
              changeFilter();
            }}
          />
        </div>
      </div>
      {attribution === "unknown" && (
        <p className="text-label text-fg-muted">这些记录未计入当前 Agent。</p>
      )}
      {resource.error && (
        <Alert status="warning">
          <AlertTitle>模型用量未更新</AlertTitle>
          <AlertDescription>
            {resource.error} {data ? "保留上次快照。" : ""}
          </AlertDescription>
        </Alert>
      )}
      <div className="flex flex-wrap gap-2">
        <Badge
          size="sm"
          status={
            stale || (data && data.sourceState !== "ready")
              ? "warning"
              : "neutral"
          }
        >
          {stale
            ? "最后快照 · 过期"
            : !data
              ? "正在读取"
              : disconnected
                ? "来源尚未接入"
                : data.sourceState === "ready"
                  ? "仅已记录用量"
                  : "来源索引中 / 部分可读"}
        </Badge>
        {data && (
          <span className="text-label text-fg-muted">
            更新 {date(data.asOf)} · 原生 / 观测快照 {date(data.sourceAsOf)}
          </span>
        )}
      </div>
      <dl className="grid gap-3 sm:grid-cols-3">
        <Summary
          label={countLabel}
          value={disconnected ? "未读取" : number(data?.summary.attempts)}
        />
        <Summary
          label="总 Token · 已报告部分"
          value={usageNumber(data?.summary, "totalTokens")}
        />
        <Summary
          label="总 Token 未报告的记录"
          value={disconnected ? "未知" : number(data?.summary.missingUsage)}
        />
      </dl>
      <CostTotalsView
        compact
        cost={disconnected ? undefined : data?.summary.cost}
        label="模型调用费用"
      />
      {data && data.missingTime > 0 && (
        <p className="text-label text-fg-muted">
          {number(data.missingTime)}{" "}
          条记录的调用时间未知；选择日期范围时不计入。
        </p>
      )}
      {data && data.items.length > 0 ? (
        <ModelUsageTable
          items={data.items}
          source={source}
          asOf={data.asOf}
          onSelect={setSelected}
        />
      ) : (
        !resource.loading &&
        !resource.error && (
          <ConnectionEmpty
            compact
            title={disconnected ? "尚未读取此原生来源" : "没有匹配的已记录用量"}
            description={
              disconnected
                ? agentId === "antigravity"
                  ? "可在连接与观测中授权只读来源；仅展示原生记录已报告的用量。"
                  : "可在连接与观测中授权只读来源；网关记录仍可独立查看。"
                : "可调整来源、Provider、模型或日期；未记录不代表没有调用。"
            }
          />
        )
      )}
      {data && (
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="text-label text-fg-muted">
            {number(data.total)} 个模型 / Provider 分组 · 第 {pages.length + 1}{" "}
            页 · 汇总包含全部匹配记录
          </p>
          <div className="flex gap-2">
            {pages.length > 0 && (
              <Button
                variant="tertiary"
                size="sm"
                onClick={() => {
                  setPages([]);
                  setSelected(null);
                }}
              >
                返回第一页
              </Button>
            )}
            {data.nextCursor && (
              <Button
                variant="tertiary"
                size="sm"
                onClick={() => {
                  setPages([...pages, data.nextCursor!]);
                  setSelected(null);
                }}
              >
                下一页
              </Button>
            )}
          </div>
        </div>
      )}
      {!data && pages.length > 0 && resource.error && (
        <Button variant="tertiary" size="sm" onClick={() => setPages([])}>
          从第一页重新读取
        </Button>
      )}
      <Accordion className="w-full">
        <AccordionItem value="scope">
          <AccordionTrigger>统计说明</AccordionTrigger>
          <AccordionContent>
            <div className="space-y-2 text-label text-fg-muted">
              <p>
                {source === "native"
                  ? "按模型响应时间统计，历史登录方式可能未知。"
                  : "按网关请求开始时间统计，包含重试与已报告用量的失败请求。"}{" "}
                两种来源可能重复，不可相加。
              </p>
              <p>
                只计入来源明确的调用；无法归属的记录可切换“同家客户端来源未知”查看。当前配置不用于分配历史用量。
              </p>
              <p>
                原生来源仅将唯一响应 ID
                对应的网关费用计入该响应；未确认关联的费用未知。网关来源按尝试计费，不与原生来源相加。
              </p>
              <p>模型用量与账户额度分开统计，订阅不折算 API 费用。</p>
            </div>
          </AccordionContent>
        </AccordionItem>
      </Accordion>
      <Dialog
        open={Boolean(selected)}
        onOpenChange={(open) => {
          if (!open) setSelected(null);
        }}
      >
        <DialogContent size="lg">
          <DialogHeader>
            <DialogTitle>
              {selected?.row.model ?? "模型未记录"} · 用量详情
            </DialogTitle>
            <DialogDescription>
              按当前 Agent、来源、Provider
              与日期范围统计。数值中的“部分”表示只有部分记录报告该字段。
            </DialogDescription>
          </DialogHeader>
          {selected && (
            <div className="space-y-4">
              <p className="break-all text-body">
                {selected.row.provider ?? "Provider 未记录"} ·{" "}
                {selected.row.endpoint ??
                  "历史 Endpoint 未记录，鉴权 / 网关归属待确认"}
              </p>
              {selected.row.routeRevision !== null && (
                <p className="text-label text-fg-muted">
                  历史路由版本 {selected.row.routeRevision} · 不按当前 Provider
                  配置回填
                </p>
              )}
              <CostTotalsView cost={selected.row.totals.cost} />
              <dl className="grid gap-3 sm:grid-cols-2">
                {usageFields.map(([key, label]) => (
                  <Summary
                    key={key}
                    label={label}
                    value={usageNumber(selected.row.totals, key)}
                  />
                ))}
              </dl>
              <p className="text-label text-fg-muted">
                {countLabel} {number(selected.row.totals.attempts)} · 总 Token
                未报告 {number(selected.row.totals.missingUsage)} · 最近记录{" "}
                {date(selected.row.lastSeen)} · 快照 {date(selected.asOf)}
              </p>
              <p className="text-label text-fg-muted">
                缓存与推理为对应输入 / 输出的子集，已经包含在总 Token
                中，不再相加。未提供的字段保留未知；本页不是账户账单。
              </p>
              {(selected.row.requestId || selected.row.sessionId) && (
                <Button
                  variant="tertiary"
                  size="sm"
                  onClick={() => {
                    window.location.hash = selected.row.requestId
                      ? `/requests/${encodeURIComponent(selected.row.requestId)}?${new URLSearchParams({ range: "all", agent: queryAgent })}`
                      : sessionLink(selected.row.sessionId!, {
                          agent: queryAgent,
                          from: agentId,
                          tab: "quota",
                        });
                  }}
                >
                  {selected.row.requestId ? "查看关联请求" : "查看关联 Session"}
                </Button>
              )}
            </div>
          )}
        </DialogContent>
      </Dialog>
    </section>
  );
}
function Summary({ label, value }: { label: string; value: string }) {
  return (
    <div className="space-y-1 rounded-lg border border-border p-3">
      <dt className="text-label text-fg-muted">{label}</dt>
      <dd className="break-words text-title font-medium">{value}</dd>
    </div>
  );
}
