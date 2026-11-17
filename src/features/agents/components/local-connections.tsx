import { useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
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
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { createReadResource } from "@/shared/lib/read-resource";
import { modelServicesResource } from "@/features/model-services/queries";
import { localConnectionsApi, type LocalBinding } from "../api";
import { BindingBudget } from "./binding-budget";

const connectionsResource = createReadResource(localConnectionsApi.list, {
  interval: false,
});
const bindingsResource = createReadResource(localConnectionsApi.budgets, {
  interval: false,
});

function Connections() {
  const connections = useSharedRead(connectionsResource);
  const bindings = useSharedRead(bindingsResource);
  const services = useSharedRead(modelServicesResource).data?.services ?? [];
  const [name, setName] = useState("");
  const [serviceId, setService] = useState<string | null>(null);
  const [bindingId, setBinding] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<{
    entry: LocalBinding;
    kind: "rotate" | "revoke";
  } | null>(null);
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const storeId = connections.data?.storeId;
  const entry = connections.data?.items.find(
    (item) => item.bindingId === bindingId,
  );
  const service = services.find((service) => service.id === serviceId);
  const op = () => ({ storeId: storeId!, operationId: crypto.randomUUID() });
  const write = async (
    input: Parameters<typeof localConnectionsApi.write>[0],
  ) => {
    setBusy(true);
    setError("");
    setStatus("");
    try {
      const result = await localConnectionsApi.write(input);
      if (result.saveError) setError(result.saveError);
      else if (result.cancelled) setStatus("已取消文件选择。");
      else if (result.receipt) {
        const receipt = result.receipt;
        setStatus(
          {
            active: "已保存并激活本机连接。",
            pending: "连接待激活，请核对原连接文件。",
            revoked: "连接已撤销。",
            cancelled: "待激活操作已取消。",
          }[receipt.state],
        );
        if (receipt.state === "active") setBinding(receipt.bindingId);
      }
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      await Promise.all([connections.invalidate(), bindings.invalidate()]);
      setBusy(false);
      setConfirm(null);
    }
  };
  const choose = (kind: "rotate" | "revoke", entry: LocalBinding) =>
    setConfirm({ kind, entry });
  return (
    <div className="space-y-4">
      {(connections.error || bindings.error || error) && (
        <p role="alert">{error || connections.error || bindings.error}</p>
      )}
      {status && <p role="status">{status}</p>}
      <section aria-label="添加本机连接" className="space-y-3">
        <h3 className="text-heading-sm font-medium">添加本机连接</h3>
        <p className="text-label text-fg-muted">
          Chat Completions · 连接文件包含 Base URL 和 Key。
        </p>
        <div className="flex flex-wrap items-center gap-2">
          <Input
            aria-label="本机连接名称"
            placeholder="连接名称"
            value={name}
            maxLength={80}
            onChange={(e) => setName(e.target.value)}
          />
          <Select value={serviceId ?? ""} onValueChange={setService}>
            <SelectTrigger aria-label="本机连接模型服务" />
            <SelectContent>
              {services.map((service) => (
                <SelectItem key={service.id} value={service.id}>
                  {service.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button
            disabled={busy || !storeId || !service || !name.trim()}
            onClick={() => {
              if (service)
                void write({
                  kind: "create",
                  query: op(),
                  name,
                  serviceId: service.id,
                  serviceVersion: service.versionId,
                });
            }}
          >
            保存连接文件并激活
          </Button>
        </div>
      </section>
      {!!connections.data?.pending.length && (
        <section aria-label="待激活连接" className="space-y-2">
          <h3 className="text-heading-sm">待激活连接</h3>
          {connections.data.pending.map((receipt) => (
            <div
              key={receipt.operationId}
              className="flex flex-wrap items-center gap-2"
            >
              <span>
                {receipt.bindingId} · Key v{receipt.keyVersion}
              </span>
              <Button
                disabled={busy}
                onClick={() =>
                  void write({
                    kind: "resume",
                    query: {
                      storeId: storeId!,
                      operationId: receipt.operationId,
                    },
                  })
                }
              >
                核对原文件并激活
              </Button>
              <Button
                disabled={busy}
                variant="tertiary"
                onClick={() =>
                  void write({
                    kind: "cancel",
                    query: {
                      storeId: storeId!,
                      operationId: receipt.operationId,
                    },
                  })
                }
              >
                取消待激活操作
              </Button>
            </div>
          ))}
        </section>
      )}
      {!!bindings.data?.items.length && (
        <Select value={bindingId ?? ""} onValueChange={setBinding}>
          <SelectTrigger aria-label="选择本机绑定详情" />
          <SelectContent>
            {bindings.data.items.map((item) => {
              const name = connections.data?.items.find(
                (entry) => entry.bindingId === item.bindingId,
              )?.name;
              return (
                <SelectItem key={item.bindingId} value={item.bindingId}>
                  {name ??
                    `${item.agentId} · ${item.model} · ${item.bindingId.slice(0, 8)}`}
                </SelectItem>
              );
            })}
          </SelectContent>
        </Select>
      )}
      {entry && (
        <section aria-label="本机连接详情" className="space-y-2">
          <h3 className="text-heading-sm">{entry.name}</h3>
          <p className="break-all text-label">
            {entry.baseUrl} · Key v{entry.keyVersion} ·{" "}
            {entry.revoked ? "已撤销" : "已激活"}
          </p>
          {!entry.revoked && (
            <div className="flex flex-wrap gap-2">
              <Button
                disabled={busy}
                variant="tertiary"
                onClick={() => choose("rotate", entry)}
              >
                轮换 Key
              </Button>
              <Button
                disabled={busy}
                variant="destructive"
                onClick={() => choose("revoke", entry)}
              >
                撤销绑定
              </Button>
            </div>
          )}
        </section>
      )}
      {bindingId && (
        <BindingBudget
          key={bindingId}
          bindingId={bindingId}
          services={services.filter(
            (service) => service.id !== entry?.serviceId,
          )}
          revoked={
            entry?.revoked ??
            bindings.data?.items.find((item) => item.bindingId === bindingId)
              ?.revoked ??
            false
          }
        />
      )}
      <Dialog
        open={confirm !== null}
        onOpenChange={(open) => {
          if (!open && !busy) setConfirm(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {confirm?.kind === "rotate" ? "轮换 Key" : "撤销绑定"}
            </DialogTitle>
            <DialogDescription>
              {confirm?.kind === "rotate"
                ? "保存新的连接文件后，旧 Key 立即失效。"
                : "现有 Key 将失效，已记录的账本与历史仍可查询。"}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setConfirm(null)}
            >
              取消
            </Button>
            <Button
              disabled={busy}
              onClick={() => {
                if (confirm)
                  void write({
                    kind: confirm.kind,
                    query: op(),
                    bindingId: confirm.entry.bindingId,
                    expectedKeyVersion: confirm.entry.keyVersion,
                  });
              }}
            >
              确认
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

export function LocalConnections() {
  const [open, setOpen] = useState(false);
  return (
    <section aria-label="本机连接" className="space-y-3">
      <Button
        variant="tertiary"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {open ? "收起本机连接与预算" : "添加本机连接 / 管理预算"}
      </Button>
      {open && <Connections />}
    </section>
  );
}
