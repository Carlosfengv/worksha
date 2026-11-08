import { useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { routingPolicyApi } from "@/features/model-services/api";
import { fullPluginsApi, pluginsApi, pluginUploadsApi } from "./api";
import { PermissionSummary } from "./controls";
import type { PluginUploadReview } from "./types";
import { backendReadEpoch } from "@/shared/lib/backend-recovery";

export type UploadedPluginIdentity = {
  kind: "plugin" | "source" | "policy";
  digest: string;
};
export function PluginAdmissionRules() {
  return (
    <details className="rounded-xl border border-border p-4 text-label">
      <summary className="cursor-pointer font-medium">
        插件标准与准入规则
      </summary>
      <ul className="mt-3 list-disc space-y-2 pl-5 text-fg-muted">
        <li>
          使用 .laplug
          插件包，声明唯一标识、发布者、版本、兼容的宿主接口、能力、依赖和所需权限。
        </li>
        <li>
          上传时验证包结构、文件摘要、签名及发布者；版本、能力或权限不受支持的包不能安装。
        </li>
        <li>
          确认来源与权限后安装并启用；需要文件、账户或模型服务时，再由你选择并授权具体资源。
        </li>
        <li>
          插件在受限环境中运行，不能自行读取本机文件或凭据。初始化失败保持未运行状态，错误结果不能冒充成功。
        </li>
        <li>“系统自带”和“用户上传”由应用记录，插件不能自行声明为系统功能。</li>
      </ul>
    </details>
  );
}
export function PluginUpload({
  storeId,
  onUploaded,
  disabled = false,
  onBusyChange,
  offerBundledDsh = false,
  showDshBridge = false,
}: {
  storeId?: string;
  onUploaded: (identity: UploadedPluginIdentity) => Promise<void>;
  disabled?: boolean;
  onBusyChange?: (busy: boolean) => void;
  offerBundledDsh?: boolean;
  showDshBridge?: boolean;
}) {
  const [review, setReview] = useState<PluginUploadReview | null>(null);
  const [bundledReview, setBundledReview] = useState(false);
  const [approve, setApprove] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const gate = useRef(false);
  const reviewEpoch = useRef<symbol | null>(null);
  const inspect = async (bundled: boolean) => {
    if (gate.current || disabled) return;
    gate.current = true;
    setBusy(true);
    onBusyChange?.(true);
    setError(null);
    setApprove(false);
    try {
      const epoch = backendReadEpoch();
      const result = bundled
        ? await pluginUploadsApi.inspectBundledDsh()
        : await pluginUploadsApi.inspect();
      if (epoch !== backendReadEpoch())
        throw Error("后台连接或功能状态已变化，请重新选择插件并核对。");
      reviewEpoch.current = epoch;
      setBundledReview(bundled);
      setReview(result);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
      onBusyChange?.(false);
      gate.current = false;
    }
  };
  const openDshBridges = async () => {
    setError(null);
    try {
      await pluginUploadsApi.openDshBridges();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  };
  const install = async () => {
    if (!review || gate.current) return;
    const requireContext = () => {
      if (reviewEpoch.current !== backendReadEpoch())
        throw Error("后台连接或功能状态已变化，未继续执行；请重新读取并核对。");
    };
    gate.current = true;
    setBusy(true);
    onBusyChange?.(true);
    setError(null);
    let installed = false;
    const identity = { kind: review.kind, digest: review.review.digest };
    try {
      requireContext();
      const pkg = review.review;
      if (review.kind === "plugin") {
        await pluginsApi.install({
          packageRef: pkg.packageRef,
          digest: pkg.digest,
          approvePublisher: approve,
        });
        installed = true;
        requireContext();
        await pluginsApi.change({
          pluginId: pkg.manifest.id,
          digest: pkg.digest,
          action: "enable",
        });
      } else if (review.kind === "source") {
        if (!pkg.signingKey) throw Error("此插件需要有效的发布者签名。");
        await fullPluginsApi.install({
          packageRef: pkg.packageRef,
          digest: pkg.digest,
          approvePublisher: approve,
          expectedSigningKey: pkg.signingKey,
        });
        installed = true;
        requireContext();
        await fullPluginsApi.change({
          package: { pluginId: pkg.manifest.id, digest: pkg.digest },
          action: "enable",
        });
      } else {
        if (!storeId)
          throw Error("当前后台的插件配置尚未读取完成，请重新读取后再安装。");
        if (!pkg.signingKey) throw Error("此插件需要有效的发布者签名。");
        await routingPolicyApi.perform({
          kind: "install",
          storeId,
          packageRef: pkg.packageRef,
          digest: pkg.digest,
          expectedSigningKey: pkg.signingKey,
          approvePublisher: approve,
        });
        installed = true;
        requireContext();
        await routingPolicyApi.perform({
          kind: "state",
          storeId,
          package: { pluginId: pkg.manifest.id, digest: pkg.digest },
          action: "enable",
        });
      }
      setReview(null);
      await onUploaded(identity);
    } catch (reason) {
      setReview(null);
      setError(
        `${installed ? "插件已安装，启用结果待核查。请在列表中重新读取状态。" : "安装结果尚未确认，请重新读取列表核对。"} ${reason instanceof Error ? reason.message : String(reason)}`,
      );
      await onUploaded(identity);
    } finally {
      setBusy(false);
      onBusyChange?.(false);
      gate.current = false;
    }
  };
  return (
    <>
      <Button
        loading={busy}
        disabled={disabled}
        onClick={() => void inspect(false)}
      >
        上传插件
      </Button>
      {offerBundledDsh && (
        <Button
          variant="tertiary"
          disabled={busy || disabled}
          onClick={() => void inspect(true)}
        >
          安装随附 DSH
        </Button>
      )}
      {showDshBridge && (
        <Button
          variant="tertiary"
          disabled={busy || disabled}
          onClick={() => void openDshBridges()}
        >
          打开 DSH 桥接文件夹
        </Button>
      )}
      {error && (
        <Alert>
          <AlertTitle>插件操作未完成</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      <Dialog
        open={!!review}
        onOpenChange={(open) => !open && !busy && setReview(null)}
      >
        <DialogContent
          size="lg"
          className="flex max-h-[calc(100dvh-2rem)] flex-col"
        >
          <DialogHeader className="shrink-0">
            <DialogTitle>确认安装并启用插件</DialogTitle>
            <DialogDescription>
              兼容性、包结构、能力声明和签名检查已通过。核对发布者与权限后，安装并启用；具体文件和账户仍需单独授权。
            </DialogDescription>
          </DialogHeader>
          {review && (
            <div className="min-h-0 space-y-4 overflow-y-auto">
              <p className="font-medium">
                {review.review.manifest.displayName} ·{" "}
                {review.review.manifest.version}
              </p>
              {reviewEpoch.current !== backendReadEpoch() && (
                <p role="alert" className="text-label text-warning-fg">
                  后台连接或功能状态已变化，请关闭弹层并重新上传核对。
                </p>
              )}
              {review.kind === "policy" && !storeId && (
                <p role="alert" className="text-label text-warning-fg">
                  后台插件配置尚未就绪，请重新读取插件目录后再安装。
                </p>
              )}
              <p className="break-all text-label text-fg-muted">
                来源：{bundledReview ? "随应用提供" : "用户上传"}
                <br />
                发布者：{review.review.manifest.publisher}
                <br />
                公钥：{review.review.signingKey ?? "未签名开发包"}
                <br />
                包摘要：{review.review.digest}
              </p>
              {review.kind === "policy" ? (
                <p className="text-label text-fg-muted">
                  对模型服务提供的有限候选进行排序，不获得文件、网络或凭据访问；启用后仍需在模型服务中明确绑定。
                </p>
              ) : (
                <PermissionSummary manifest={review.review.manifest} />
              )}
              {review.review.signingKey && !review.review.trustedPublisher && (
                <label className="flex items-start gap-2 text-label">
                  <Checkbox
                    checked={approve}
                    onCheckedChange={(value) => setApprove(value === true)}
                  />
                  我已核对并信任此发布者公钥
                </label>
              )}
              {review.review.trustedPublisher && (
                <p className="text-label text-fg-muted">
                  此公钥已在当前数据目录受信任。
                </p>
              )}
            </div>
          )}
          <DialogFooter className="shrink-0">
            <Button
              variant="ghost"
              disabled={busy}
              onClick={() => setReview(null)}
            >
              取消
            </Button>
            <Button
              loading={busy}
              disabled={
                !review ||
                reviewEpoch.current !== backendReadEpoch() ||
                (review.kind === "policy" && !storeId) ||
                (!!review.review.signingKey &&
                  !review.review.trustedPublisher &&
                  !approve)
              }
              onClick={() => void install()}
            >
              安装并启用
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
