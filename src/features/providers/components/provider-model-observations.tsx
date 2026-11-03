import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { observationFields, observationStatus } from "../model-observations";
import type { InventoryRow } from "../model-inventory";

export function ProviderModelObservations({ row }: { row: InventoryRow }) {
  const observations = row.observations;
  if (!observations.length)
    return (
      <p className="min-w-48 text-label text-fg-muted">
        实测未知 ·
        {row.catalog?.data?.observations?.state === "unavailable"
          ? "记录读取失败"
          : "暂无匹配的实测记录"}
      </p>
    );
  return (
    <div className="min-w-56 max-w-72 space-y-1 whitespace-normal text-label">
      <p className="text-fg-muted">
        {observations[0].checkedAt.slice(0, 10)} · 本次实测
      </p>
      <dl className="space-y-1">
        {observationFields.slice(0, 5).map(([field, label]) => (
          <div key={field}>
            <dt className="inline text-fg-muted">{label}：</dt>
            <dd className="inline">{observationStatus(observations, field)}</dd>
          </div>
        ))}
      </dl>
      <Dialog>
        <DialogTrigger render={<Button variant="link" size="sm" />}>
          实测详情
        </DialogTrigger>
        <DialogContent size="lg">
          <DialogHeader>
            <DialogTitle>模型实测记录</DialogTitle>
            <DialogDescription>
              {row.provider.name} · {row.model.id}
            </DialogDescription>
          </DialogHeader>
          <div className="max-h-96 space-y-5 overflow-y-auto">
            <p className="text-body text-fg-muted">
              仅代表所列客户端、连接和日期的样本。官方声明单独展示，未知项目未获验证。
              使用临时配置直接访问 Provider，未验证生产网关转换或计入网关用量。
            </p>
            {observations.map((o, index) => (
              <section key={index} className="space-y-2">
                <h3 className="text-body font-medium">
                  {o.client} {o.clientVersion} · {o.checkedAt.slice(0, 10)}
                </h3>
                <dl className="space-y-3 text-body">
                  {observationFields.map(([field, label]) => {
                    const check = o.checks.find((c) => c.field === field);
                    return (
                      <div key={field}>
                        <dt className="font-medium">
                          {label}：{observationStatus([o], field)}
                        </dt>
                        <dd className="text-fg-muted">
                          {check?.detail ?? "本轮未验证，保留未知。"}
                        </dd>
                      </div>
                    );
                  })}
                </dl>
              </section>
            ))}
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
