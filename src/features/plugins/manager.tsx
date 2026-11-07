import { useRef, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { Provider } from "@/features/providers/types";
import { useSharedRead, useSharedQuery } from "@/shared/hooks/use-shared-read";
import {
  modelServicesResource,
  routingPoliciesResource,
} from "@/features/model-services/queries";
import { routingPolicyApi } from "@/features/model-services/api";
import {
  pluginDirectoryResource,
  fullPluginDirectoryResource,
  builtinPluginResource,
} from "./queries";
import { pluginsApi, fullPluginsApi } from "./api";
import { PluginControls } from "./controls";
import { PluginOptionalDependencies } from "./optional-dependencies";
import { PluginOperationHistory } from "./advanced-controls";
import { PluginDiagnosticsPanel } from "./diagnostics-panel";
import { FullPluginManager } from "./full-manager";
import { BuiltinPluginManager, BuiltinSupportTable } from "./builtin-manager";
import {
  PluginCenterCatalog,
  type CenterSelection,
  type CenterUserState,
} from "./center-catalog";
import {
  PluginAdmissionRules,
  PluginUpload,
  type UploadedPluginIdentity,
} from "./upload";
import { systemPluginPresentation } from "./scenarios";
import { systemAgentPlugins, type SystemFeature } from "./system-features";
import type { StateAction } from "./types";
import { backendReadEpoch } from "@/shared/lib/backend-recovery";
import { pluginViewHref } from "./navigation";

type ConfirmationInput = CenterUserState & { action?: StateAction };
type Confirmation = ConfirmationInput & { epoch: symbol; storeId?: string };
const actionNames: Record<StateAction, string> = {
  enable: "启用",
  disable: "停用",
  remove: "卸载此版本",
  rollback: "回退上一版本",
  "revoke-grants": "撤销资源授权",
  "revoke-publisher": "撤销发布者信任",
};
export function PluginManager({
  providers,
  initialScene,
  systemFeatures = [],
  refreshSnapshot,
}: {
  providers: Provider[];
  initialScene?: string;
  systemFeatures?: SystemFeature[];
  refreshSnapshot?: () => Promise<void>;
}) {
  const general = useSharedRead(pluginDirectoryResource);
  const source = useSharedRead(fullPluginDirectoryResource);
  const builtinState = useSharedRead(builtinPluginResource);
  const features = [
    ...systemFeatures,
    ...systemAgentPlugins(builtinState.data?.host.plugins ?? []),
  ];
  const models = useSharedRead(
    modelServicesResource,
    !builtinState.data ||
      !!builtinState.data.host.plugins.find(
        (p) => p.id === "builtin.model-services",
      )?.runtimeActive,
  );
  const storeId =
    builtinState.data?.storeId ??
    (models.error ? undefined : models.data?.writeProtocol?.storeId);
  const policy = useSharedQuery(
    routingPoliciesResource,
    storeId ?? "",
    !!storeId,
  );
  const [catalogReset, setCatalogReset] = useState(0);
  const [selected, setSelected] = useState<CenterSelection | null>(null);
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const gate = useRef(false);
  const confirm = (input: ConfirmationInput) =>
    setConfirmation({ ...input, epoch: backendReadEpoch(), storeId });
  const refresh = async () => {
    await Promise.all([
      builtinPluginResource.invalidate(),
      models.invalidate(),
      general.invalidate(),
      source.invalidate(),
      ...(storeId ? [policy.invalidate()] : []),
    ]);
  };
  const uploaded = async (identity: UploadedPluginIdentity) => {
    await refresh();
    setSelected({ kind: identity.kind, id: identity.digest });
    setCatalogReset((value) => value + 1);
  };
  const change = async () => {
    if (!confirmation || gate.current) return;
    gate.current = true;
    setBusy(true);
    setError(null);
    const action =
      confirmation.action ?? (confirmation.enabled ? "enable" : "disable");
    const pkg = {
      pluginId: confirmation.pluginId,
      digest: confirmation.digest,
    };
    try {
      if (confirmation.epoch !== backendReadEpoch())
        throw Error("后台连接或功能状态已变化，请重新读取插件并核对操作。");
      if (confirmation.kind === "plugin")
        await pluginsApi.change({ ...pkg, action });
      else if (confirmation.kind === "source")
        await fullPluginsApi.change({ package: pkg, action });
      else {
        if (!storeId || storeId !== confirmation.storeId)
          throw Error("后台插件配置尚未读取，请刷新后重试。");
        await routingPolicyApi.perform({
          kind: "state",
          storeId,
          package: pkg,
          action,
        });
      }
      setConfirmation(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      setConfirmation(null);
    } finally {
      await refresh();
      setBusy(false);
      gate.current = false;
    }
  };
  const current =
    selected?.kind === "plugin"
      ? general.data?.items.find((p) => p.digest === selected.id)
      : undefined;
  const currentPolicy =
    selected?.kind === "policy"
      ? policy.data?.packages.find((p) => p.digest === selected.id)
      : undefined;
  const confirmationUnavailable =
    !!confirmation &&
    (confirmation.epoch !== backendReadEpoch() ||
      (confirmation.kind === "plugin" && !!general.error) ||
      (confirmation.kind === "source" && !!source.error) ||
      (confirmation.kind === "policy" &&
        (!!policy.error || !storeId || storeId !== confirmation.storeId)));
  const problems = [
    !models.error &&
      !models.loading &&
      !storeId &&
      "当前后台未提供完整的插件目录兼容信息",
    general.error && `上传插件目录：${general.error}`,
    source.error && `数据插件目录：${source.error}`,
    (models.error || policy.error) &&
      `策略插件目录：${models.error ?? policy.error}`,
  ].filter(Boolean);
  return (
    <section className="space-y-4" aria-label="插件中心">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="space-y-1">
          <h2 className="text-title font-medium">插件中心</h2>
          <p className="text-label text-fg-muted">
            管理系统自带和用户上传的功能。启停或卸载前核对依赖与受影响的服务，确认后生效。
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <PluginUpload
            storeId={storeId}
            onUploaded={uploaded}
            disabled={busy || !!confirmation}
            onBusyChange={setBusy}
          />
          <Button
            variant="tertiary"
            disabled={busy}
            onClick={() => void refresh()}
          >
            重新读取插件
          </Button>
        </div>
      </div>
      <PluginAdmissionRules />
      {general.data?.developmentMode && (
        <p className="text-label text-warning-fg">
          当前为显式插件开发模式，允许未签名开发包。
        </p>
      )}
      {problems.length > 0 && (
        <Alert>
          <AlertTitle>部分插件目录暂不可用</AlertTitle>
          <AlertDescription>
            {problems.join("；")}。保留已读取条目，不能视为完整列表。
          </AlertDescription>
        </Alert>
      )}
      {error && (
        <Alert>
          <AlertTitle>插件操作未完成</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      <BuiltinPluginManager
        refreshSnapshot={refreshSnapshot}
        externalBusy={busy}
        renderCatalog={(system, loading) => (
          <>
            <PluginCenterCatalog
              key={catalogReset}
              features={features}
              systemRows={system}
              initialScene={catalogReset ? undefined : initialScene}
              plugins={general.data?.items ?? []}
              sources={source.data?.items ?? []}
              policies={policy.data?.packages ?? []}
              loading={
                loading ||
                general.loading ||
                source.loading ||
                models.loading ||
                policy.loading
              }
              uploadsUnavailable={!!general.error || general.loading}
              sourceUnavailable={!!source.error || source.loading}
              policyUnavailable={!!policy.error || !storeId || policy.loading}
              busy={busy || !!confirmation}
              selected={selected}
              onSelect={setSelected}
              onUserToggle={confirm}
            />
            {selected?.kind === "system" &&
              features
                .filter((feature) => feature.id === selected.id)
                .map((feature) => (
                  <article
                    key={feature.id}
                    className="space-y-3 rounded-xl border border-border p-4"
                    aria-label="功能详情"
                  >
                    <h3 className="text-title font-medium">{feature.name}</h3>
                    <p className="text-body">{feature.purpose}</p>
                    <p className="text-label text-fg-muted">
                      使用条件：{feature.conditions}
                    </p>
                    <p className="text-label font-medium">
                      进入「{feature.name}」页面后可使用的操作
                    </p>
                    <ul className="list-disc space-y-1 pl-5 text-label">
                      {feature.operations.map((operation) => (
                        <li key={operation}>{operation}</li>
                      ))}
                    </ul>
                    {system
                      .filter((row) => row.plugin.id === feature.id)
                      .map((row) => (
                        <div
                          className="flex flex-wrap gap-2"
                          key={row.plugin.id}
                        >
                          {row.plugin.runtimeActive && (
                            <Button variant="secondary" asChild>
                              <a href={feature.href}>打开{feature.name}</a>
                            </Button>
                          )}
                          <Button
                            variant="tertiary"
                            disabled={row.disabled || busy}
                            onClick={() =>
                              row.installed
                                ? row.toggle(!row.plugin.runtimeActive)
                                : row.install()
                            }
                          >
                            {row.installed
                              ? row.plugin.runtimeActive
                                ? "停用"
                                : "启用"
                              : "安装并启用"}
                          </Button>
                          {row.installed && (
                            <Button
                              variant="tertiary"
                              disabled={row.disabled || busy}
                              onClick={row.remove}
                            >
                              卸载
                            </Button>
                          )}
                        </div>
                      ))}
                    <p className="text-label text-fg-muted">
                      启停和卸载会先检查依赖。卸载保留历史数据与配置，系统自带版本可重新安装；不会改动原始文件或终止外部
                      Agent。
                    </p>
                    {feature.supportScenes.length > 0 && (
                      <details className="space-y-3 text-label">
                        <summary className="cursor-pointer">
                          内部支撑（开发者信息）
                        </summary>
                        <p className="text-fg-muted">
                          以下为当前功能相关的只读支撑。停用业务功能会回收归属它的支撑；共享数据与其他功能按依赖分别处理。
                        </p>
                        <BuiltinSupportTable
                          rows={system.filter((row) =>
                            systemPluginPresentation(row.plugin).scenes.some(
                              (scene) => feature.supportScenes.includes(scene),
                            ),
                          )}
                        />
                      </details>
                    )}
                  </article>
                ))}
          </>
        )}
      />
      {current && (
        <article
          className="space-y-4 rounded-xl border border-border p-4"
          aria-label="插件详情"
        >
          <h3 className="text-title font-medium">
            {current.manifest.displayName} · {current.manifest.version}
          </h3>
          <p className="text-label text-fg-muted">
            用户上传 · 发布者：{current.manifest.publisher}
          </p>
          <fieldset disabled={!!general.error}>
            <PluginControls
              key={current.digest}
              status={current}
              providers={providers}
              onChanged={() => void refresh()}
            />
          </fieldset>
          <PluginOptionalDependencies
            status={current}
            catalog={general.data?.contributions}
            error={general.data?.contributionsError}
          />
          <PluginDiagnosticsPanel
            status={current}
            diagnostics={general.data?.diagnostics}
            unavailable={!!general.error || !!general.data?.diagnosticsError}
          />
          <div className="flex flex-wrap gap-3">
            {current.manifest.views?.map((view) => (
              <Button key={view.id} variant="link" asChild>
                <a href={pluginViewHref(current.manifest.id, view.id)}>
                  打开{view.title}
                </a>
              </Button>
            ))}
          </div>
          <details className="text-label">
            <summary className="cursor-pointer">开发者信息</summary>
            <p className="break-all text-fg-muted">
              {current.manifest.id}
              <br />
              摘要：{current.digest}
              <br />
              公钥：{current.signingKey ?? "未签名开发包"}
            </p>
          </details>
        </article>
      )}
      {selected?.kind === "source" && (
        <FullPluginManager
          key={selected.id}
          selectedDigest={selected.id}
          detailsOnly
        />
      )}
      {currentPolicy && (
        <article
          className="space-y-3 rounded-xl border border-border p-4"
          aria-label="插件详情"
        >
          <h3 className="text-title font-medium">
            {currentPolicy.manifest.displayName} ·{" "}
            {currentPolicy.manifest.version}
          </h3>
          <p className="text-label text-fg-muted">
            用户上传 · 发布者：{currentPolicy.manifest.publisher}
            。仅对有限模型候选排序；启用后需在模型服务中绑定。
          </p>
          <div className="flex flex-wrap gap-2">
            {(Object.keys(actionNames) as StateAction[]).map((action) => (
              <Button
                key={action}
                variant="tertiary"
                disabled={busy || !storeId || !!policy.error}
                onClick={() =>
                  confirm({
                    kind: "policy",
                    pluginId: currentPolicy.manifest.id,
                    digest: currentPolicy.digest,
                    enabled: action === "enable",
                    action,
                  })
                }
              >
                {actionNames[action]}
              </Button>
            ))}
            <Button variant="link" asChild>
              <a href="#/model-services">配置模型服务绑定</a>
            </Button>
          </div>
        </article>
      )}
      <PluginOperationHistory />
      <Dialog
        open={!!confirmation}
        onOpenChange={(open) => !open && !busy && setConfirmation(null)}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              确认
              {confirmation
                ? actionNames[
                    confirmation.action ??
                      (confirmation.enabled ? "enable" : "disable")
                  ]
                : "插件操作"}
            </DialogTitle>
            <DialogDescription>
              启用时检查兼容性并初始化。停用或卸载后，此插件的入口和数据会从业务页面隐藏，原文件和账户保留。停用、卸载或撤权会撤销相关资源授权。撤销发布者信任也会影响该发布者的其他插件。
            </DialogDescription>
          </DialogHeader>
          {confirmationUnavailable && (
            <p role="alert" className="text-label text-warning-fg">
              插件目录或后台已变化，请关闭弹层并重新读取后核对。
            </p>
          )}
          <DialogFooter>
            <Button
              variant="ghost"
              disabled={busy}
              onClick={() => setConfirmation(null)}
            >
              取消
            </Button>
            <Button
              loading={busy}
              disabled={confirmationUnavailable}
              onClick={() => void change()}
            >
              确认操作
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
