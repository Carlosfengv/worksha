import { useMemo, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import { providerUsageResource } from "../queries";
import { protocolName } from "../display";
import { useModelCatalogs } from "../hooks/use-model-catalogs";
import { catalogKey, modelInventory } from "../model-inventory";
import type { Provider } from "../types";
import { ProviderModelTable } from "./provider-model-table";
import { ProviderUsageNotes, ProviderUsageStatus } from "./provider-history";
import {
  ProviderUsageFilters,
  type UsageFilters,
} from "./provider-usage-filters";

export function ProviderModelInventory({
  providers,
  active = true,
  storeId,
}: {
  providers: Provider[];
  active?: boolean;
  storeId?: string;
}) {
  const [filters, setFilters] = useState<UsageFilters>({
    source: "gateway",
    days: 7,
  });
  const [search, setSearch] = useState("");
  const [providerId, setProviderId] = useState("all");
  const [availability, setAvailability] = useState("all");
  const configured = useMemo(
    () => providers.filter((p) => p.configured),
    [providers],
  );
  const { catalogs, reading, available, readAll } = useModelCatalogs(
    configured,
    storeId,
    active,
  );
  const usage = useSharedQuery(
    providerUsageResource,
    filters,
    active && configured.length > 0,
  );
  const rows = useMemo(
    () => modelInventory(configured, catalogs, usage.data),
    [configured, catalogs, usage.data],
  );
  const query = search.trim().toLowerCase();
  const selectedProvider = configured.some((p) => p.id === providerId)
    ? providerId
    : "all";
  const matching = useMemo(
    () =>
      rows.filter(
        (row) =>
          (selectedProvider === "all" ||
            row.provider.id === selectedProvider) &&
          (availability === "all" ||
            (availability === "enabled"
              ? row.state === "已启用"
              : row.state === "已停用")) &&
          (!query ||
            `${row.model.id} ${row.model.name} ${row.provider.name}`
              .toLowerCase()
              .includes(query)),
      ),
    [rows, selectedProvider, availability, query],
  );
  if (!configured.length)
    return (
      <ConnectionEmpty
        title="尚未添加 Provider"
        description="添加连接后，可在这里比较所有模型的上下文、能力和用量。"
        icon="globe"
      />
    );
  return (
    <section className="space-y-4" aria-label="Provider 全部模型目录">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="space-y-1">
          <h2 className="text-title font-medium">全部模型</h2>
          <p className="text-body text-fg-muted">
            汇集 {configured.length} 个 Provider
            的已登记模型与读取到的目录，按连接分别统计用量。
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="secondary"
            loading={reading}
            disabled={reading || !available}
            onClick={() => void readAll()}
          >
            读取全部模型目录
          </Button>
          <Button
            variant="tertiary"
            loading={usage.loading}
            onClick={usage.reload}
          >
            刷新用量
          </Button>
        </div>
      </div>
      {!available && (
        <Alert status="warning">
          <AlertTitle>模型目录来源尚未核对</AlertTitle>
          <AlertDescription>
            当前后台未提供数据库身份，请刷新原后台后重新读取；已登记模型仍保留。
          </AlertDescription>
        </Alert>
      )}
      <p className="text-label text-fg-subtle">
        读取目录不会生成模型内容。上下文与能力取自目录及已核对的官方资料；结构化输出指
        JSON Schema，JSON
        模式单独列示。实测状态按连接、客户端和日期单列，样本通过不代表稳定性或容量保证；未验证项目保留未知。
      </p>
      {configured
        .filter(
          (p) =>
            catalogs[catalogKey(p)]?.error ||
            catalogs[catalogKey(p)]?.data?.truncated ||
            catalogs[catalogKey(p)]?.data?.models.length === 0,
        )
        .map((p) => {
          const state = catalogs[catalogKey(p)];
          return (
            <Alert key={p.id} status="warning">
              <AlertTitle>
                {p.name} · {protocolName(p.protocol)}
              </AlertTitle>
              <AlertDescription>
                {state.error
                  ? state.data
                    ? `${state.error}。保留上次目录和已登记模型，可重新读取。`
                    : `${state.error}。已登记模型仍保留，可重新读取目录。`
                  : state.data?.truncated
                    ? "目录达到读取上限，当前为部分结果；已登记模型仍完整保留。"
                    : "服务商返回空目录，已登记模型仍保留。"}
              </AlertDescription>
            </Alert>
          );
        })}
      <div className="flex flex-wrap items-end gap-3">
        <Field className="w-full sm:max-w-xs">
          <FieldLabel>搜索模型或 Provider</FieldLabel>
          <Input
            value={search}
            placeholder="模型名称、ID 或连接名称"
            onChange={(e) => setSearch(e.target.value)}
          />
        </Field>
        <Select value={selectedProvider} onValueChange={setProviderId}>
          <SelectTrigger aria-label="模型 Provider 筛选" />
          <SelectContent>
            <SelectItem value="all">全部 Providers</SelectItem>
            {configured.map((p) => (
              <SelectItem key={p.id} value={p.id}>
                {p.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={availability} onValueChange={setAvailability}>
          <SelectTrigger aria-label="模型可用状态筛选" />
          <SelectContent>
            <SelectItem value="all">全部模型状态</SelectItem>
            <SelectItem value="enabled">已启用</SelectItem>
            <SelectItem value="disabled">已停用</SelectItem>
          </SelectContent>
        </Select>
      </div>
      <ProviderUsageFilters value={filters} onChange={setFilters} />
      <ProviderUsageStatus
        data={usage.data}
        error={usage.error}
        reload={usage.reload}
      />
      {usage.data && usage.data.missingTime > 0 && (
        <p className="text-label text-fg-muted">
          {usage.data.missingTime.toLocaleString("zh-CN")} 次调用缺少可靠时间
          {filters.days ? "，未计入所选日期范围。" : "，保留在全部日期统计中。"}
        </p>
      )}
      {usage.data && !usage.data.providerModels && (
        <p className="text-label text-fg-muted">
          当前服务尚未提供按连接划分的模型汇总，用量暂不可用。
        </p>
      )}
      <p role="status" className="text-label text-fg-muted">
        显示 {matching.length} / {rows.length} 个模型条目 · 同名模型按 Provider
        分开
      </p>
      {matching.length ? (
        <ProviderModelTable
          key={JSON.stringify([query, selectedProvider, availability])}
          rows={matching}
        />
      ) : (
        <ConnectionEmpty
          compact
          title={rows.length ? "没有匹配的模型" : "尚未登记模型"}
          description={
            rows.length
              ? "调整搜索词或筛选条件。"
              : "读取全部模型目录，或在连接中手动添加模型。"
          }
        />
      )}
      <ProviderUsageNotes />
    </section>
  );
}
