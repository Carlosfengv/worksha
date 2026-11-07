import { useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldLabel } from "@/components/ui/field";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import type { Provider } from "@/features/providers/types";
import { pluginsApi } from "./api";
import { isExtension } from "./capabilities";
import { AdvancedControls } from "./advanced-controls";
import { ProviderCatalogControls } from "./provider-catalog-controls";
import type { Manifest, PluginStatus, StateAction } from "./types";
import { DomainConsentControls } from "@/features/domain/consent-controls";
import { domainDirectoryResource } from "@/features/domain/queries";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { pluginDirectoryResource } from "./queries";

export function PermissionSummary({ manifest }: { manifest: Manifest }) {
  if (isExtension(manifest))
    return (
      <p className="text-label text-fg-muted">
        {manifest.capabilities.includes("gateway.protocol@1")
          ? "显式绑定连接后读取并转换完整模型请求与响应；宿主负责凭据和网络。"
          : manifest.capabilities.includes("config.plan-provider@1")
            ? "为你选择的单个 JSON 配置文件生成有限字段修改；预览后确认执行，备份与恢复由宿主负责。"
            : "仅在明确启动后连接你选择的 MCP 服务。stdio 程序以当前用户权限运行；此界面不主动执行工具。"}
      </p>
    );
  return (
    <ul className="space-y-2 text-label text-fg-muted">
      {manifest.permissions?.map((permission, index) => (
        <li key={index}>
          {permission.kind === "source.content.read"
            ? "读取你选择的单个 JSONL 文件，包括全部会话正文；不读取其他目录。"
            : permission.kind === "connection.request"
              ? `向你绑定的连接发送 GET ${permission.path}；不向插件提供原始凭据。`
              : permission.kind === "domain.query"
                ? permission.query === "object.summary@1"
                  ? "在对象详情中读取单独授权的元数据摘要；具体字段、用途与版本须逐项确认。"
                  : permission.query === "domain.summary"
                    ? "通过宿主调用签名数据绑定所指定的会话来源，读取已授权的会话数量和用量；不读取正文或凭据。来源停用或撤权后立即失效。"
                    : "读取当前数据目录最近 24 小时的网关汇总；不读取原生会话、正文或凭据。"
                : "在宿主提供的扩展视图中显示内容。"}
        </li>
      ))}
    </ul>
  );
}
type Confirmation = StateAction | "authorize-report";
const actionNames: Record<Confirmation, string> = {
  "authorize-report": "授权报表汇总",
  enable: "启用此版本",
  disable: "停用",
  remove: "卸载此版本",
  rollback: "回退上一版本",
  "revoke-grants": "撤销资源授权",
  "revoke-publisher": "撤销发布者信任",
};
export function PluginControls({
  status,
  providers,
  onChanged,
}: {
  status: PluginStatus;
  providers: Provider[];
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<Confirmation | null>(null);
  const [providerId, setProviderId] = useState<string | null>(null);
  const [allowPrivate, setAllowPrivate] = useState(false);
  const [settings, setSettings] = useState(status.settings);
  const directory = useSharedRead(pluginDirectoryResource);
  const dataQueries =
    directory.data?.contributions?.items.find(
      (item) =>
        item.pluginId === status.manifest.id && item.digest === status.digest,
    )?.dataQueries ?? [];
  const run = async (operation: () => Promise<unknown>, success: string) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await operation();
      await domainDirectoryResource.invalidate();
      setNotice(success);
      onChanged();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  const source = status.manifest.permissions?.some(
    (p) => p.kind === "source.content.read",
  );
  const network = status.manifest.permissions?.some(
    (p) => p.kind === "connection.request",
  );
  const catalog = status.manifest.capabilities.includes("provider.models@1");
  const objects =
    status.manifest.requires?.includes("host.object-detail-slots@1") ?? false;
  const connectionNames = new Map(
    providers.map((p) => [p.id, `${p.name} · ${p.endpoint}`]),
  );
  return (
    <section
      className="space-y-4"
      aria-label={`${status.manifest.displayName} 插件管理`}
    >
      <PermissionSummary manifest={status.manifest} />
      {(catalog || (status.previousDigest && !objects)) && (
        <ProviderCatalogControls
          status={status}
          providers={providers}
          onChanged={onChanged}
        />
      )}
      {status.enabled && source && (
        <DomainConsentControls
          key={`${status.digest}:${status.grantRevision}`}
          pluginId={status.manifest.id}
          digest={status.digest}
          onChanged={onChanged}
        />
      )}
      {isExtension(status.manifest) && (
        <AdvancedControls
          status={status}
          providers={providers}
          onChanged={onChanged}
        />
      )}
      {!catalog && !objects && (
        <p className="break-all text-label text-fg-muted">
          来源文件：{status.sourceLabel ?? "未授权"}
          <br />
          连接：{status.connectionLabel ?? "未绑定"}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        {status.enabled &&
          status.manifest.permissions?.some(
            (p) =>
              p.kind === "domain.query" &&
              (p.query === "gateway.summary" || p.query === "domain.summary"),
          ) && (
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setConfirm("authorize-report")}
            >
              {status.manifest.requires?.includes("host.domain-services@1")
                ? status.reportAuthorized
                  ? "重新授权来源数据调用"
                  : "授权来源数据调用"
                : status.reportAuthorized
                  ? "重新授权报表汇总"
                  : "授权报表汇总"}
            </Button>
          )}
        <Button
          variant={status.enabled ? "tertiary" : "neutral"}
          disabled={busy}
          onClick={() => setConfirm(status.enabled ? "disable" : "enable")}
        >
          {status.enabled ? "停用" : "启用此版本"}
        </Button>
        {status.enabled && source && (
          <Button
            variant="tertiary"
            disabled={busy}
            onClick={() =>
              void run(
                () =>
                  pluginsApi.authorizeSource(status.manifest.id, status.digest),
                "来源选择已完成。取消选择不会更改授权。",
              )
            }
          >
            选择并授权 JSONL
          </Button>
        )}
        {status.enabled && (
          <Button
            variant="tertiary"
            disabled={busy}
            onClick={() => setConfirm("revoke-grants")}
          >
            撤销资源授权
          </Button>
        )}
        {status.previousDigest && (
          <Button
            variant="tertiary"
            disabled={busy}
            onClick={() => setConfirm("rollback")}
          >
            回退上一版本
          </Button>
        )}
        <Button
          variant="tertiary"
          disabled={busy}
          onClick={() => setConfirm("remove")}
        >
          卸载此版本
        </Button>
        {status.signingKey && (
          <Button
            variant="tertiary"
            disabled={busy}
            onClick={() => setConfirm("revoke-publisher")}
          >
            撤销发布者信任
          </Button>
        )}
      </div>
      {status.enabled && network && !catalog && (
        <div className="space-y-3">
          <Field>
            <FieldLabel>额度连接</FieldLabel>
            <Combobox<string>
              items={[...connectionNames.keys()]}
              value={providerId}
              limit={200}
              itemToStringLabel={(id) => connectionNames.get(id) ?? id}
              onValueChange={setProviderId}
            >
              <ComboboxInput
                aria-label="绑定额度连接"
                placeholder="搜索本机已有连接"
                triggerAriaLabel="展开连接目录"
              />
              <ComboboxContent>
                <ComboboxEmpty>请先在 Providers 中添加连接。</ComboboxEmpty>
                <ComboboxList>
                  {(id: string) => (
                    <ComboboxItem key={id} value={id}>
                      {connectionNames.get(id)}
                    </ComboboxItem>
                  )}
                </ComboboxList>
              </ComboboxContent>
            </Combobox>
          </Field>
          <label className="flex items-center gap-2 text-label">
            <Checkbox
              checked={allowPrivate}
              onCheckedChange={(checked) => setAllowPrivate(checked === true)}
            />
            允许此连接访问本机或局域网地址
          </label>
          {providerId && (
            <p className="break-all text-label text-fg-muted">
              将授权：{connectionNames.get(providerId)}；请求路径见上方权限。
            </p>
          )}
          <Button
            variant="tertiary"
            disabled={busy || !providerId}
            onClick={() =>
              providerId &&
              void run(
                () =>
                  pluginsApi.action({
                    pluginId: status.manifest.id,
                    digest: status.digest,
                    action: {
                      kind: "bind-connection",
                      providerId,
                      allowPrivateNetwork: allowPrivate,
                    },
                  }),
                "连接已绑定。手动刷新后才会请求额度。",
              )
            }
          >
            授权绑定连接
          </Button>
        </div>
      )}
      {(status.manifest.settings?.length ?? 0) > 0 && (
        <div className="space-y-3">
          {status.manifest.settings?.map((setting) => (
            <Field key={setting.key}>
              <FieldLabel>{setting.label}</FieldLabel>
              <Select
                value={settings[setting.key] ?? setting.default}
                onValueChange={(value) =>
                  value && setSettings({ ...settings, [setting.key]: value })
                }
              >
                <SelectTrigger aria-label={setting.label} />
                <SelectContent>
                  {setting.options.map((option) => (
                    <SelectItem key={option} value={option}>
                      {option}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
          ))}
          <Button
            variant="tertiary"
            disabled={busy}
            onClick={() =>
              void run(
                () =>
                  pluginsApi.action({
                    pluginId: status.manifest.id,
                    digest: status.digest,
                    action: { kind: "save-settings", values: settings },
                  }),
                "插件设置已保存。",
              )
            }
          >
            保存插件设置
          </Button>
        </div>
      )}
      {error && (
        <Alert status="danger">
          <AlertTitle>未能完成</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      {notice && (
        <p role="status" className="text-label text-fg-muted">
          {notice}
        </p>
      )}
      <Dialog
        open={confirm !== null}
        onOpenChange={(open) => !busy && !open && setConfirm(null)}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {confirm ? actionNames[confirm] : "插件操作"}
            </DialogTitle>
            <DialogDescription>
              {confirm === "enable"
                ? "启用将运行此包的受限初始化检查。安装和启用不会自动授权来源、请求额度或调用模型。"
                : confirm === "authorize-report"
                  ? status.manifest.permissions?.some(
                      (p) =>
                        p.kind === "domain.query" &&
                        p.query === "domain.summary",
                    )
                    ? "允许插件通过宿主调用其签名数据绑定中的会话来源，读取会话数量和用量。各来源仍须独立授权；不读取正文、请求内容或凭据。来源停用或撤权后旧结果不可继续读取。"
                    : "允许此声明式报表读取当前数据目录最近 24 小时的网关汇总。不会读取原生会话、正文、请求 ID 或凭据；授权后仍需手动刷新。"
                  : confirm === "revoke-publisher"
                    ? "同一发布者的插件将停用并撤销资源授权；以后安装或启用需要重新核对公钥。"
                    : confirm === "rollback"
                      ? "准备上一版本后再切换；原设置快照保留，资源需要重新授权。准备失败时当前版本继续可用。"
                      : "停止新任务并撤销相应授权，保留现有业务历史、原始会话与配置。"}
            </DialogDescription>
          </DialogHeader>
          <PermissionSummary manifest={status.manifest} />
          {dataQueries.length > 0 && (
            <ul className="space-y-2 text-label text-fg-muted">
              {dataQueries.map((query) => (
                <li key={query.viewId}>
                  调用来源：{query.sourceId} · {query.capability}
                </li>
              ))}
            </ul>
          )}
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setConfirm(null)}
            >
              取消
            </Button>
            <Button
              loading={busy}
              onClick={() =>
                confirm &&
                void run(async () => {
                  if (confirm === "authorize-report")
                    await pluginsApi.action({
                      pluginId: status.manifest.id,
                      digest: status.digest,
                      action: { kind: "authorize-report" },
                    });
                  else
                    await pluginsApi.change({
                      pluginId: status.manifest.id,
                      digest: status.digest,
                      action: confirm,
                    });
                  setConfirm(null);
                }, "插件状态已更新。")
              }
            >
              确认{confirm ? actionNames[confirm] : "操作"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
