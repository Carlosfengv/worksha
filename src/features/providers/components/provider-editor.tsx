import { useId } from "react";
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
import { FieldGroup, Fieldset } from "@/components/ui/field";
import {
  useProviderEditor,
  type ProviderEditorProps,
} from "../hooks/use-provider-editor";
import { ProviderCatalogSourceFields } from "./provider-catalog-source-fields";
import { ProviderConnectionFields } from "./provider-connection-fields";
import { ProviderModelFields } from "./provider-model-fields";
import { ProviderWriteStatus } from "./provider-write-status";
import { CredentialRecovery } from "./credential-recovery";
import { CredentialDiscovery } from "./credential-discovery";
import { CredentialNamespaceRecovery } from "./credential-namespace-recovery";

export function ProviderEditor(props: ProviderEditorProps) {
  const editor = useProviderEditor(props);
  const recoveryId = useId();
  const {
    form,
    dispatch,
    flags,
    credentials,
    write,
    actions,
    committed,
    shownError,
  } = editor;
  const automatic = form.mode === "automatic";
  const recoveryDisabled =
    flags.saving ||
    flags.fetching ||
    flags.presetResolving ||
    flags.sourceUnavailable;
  const pendingCredential = flags.credentialBlocked;
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !flags.busy) props.onClose();
      }}
    >
      <DialogContent
        size="lg"
        className="max-h-[calc(100dvh-2rem)] overflow-y-auto"
      >
        <DialogHeader>
          <DialogTitle>
            {props.provider.configured
              ? `配置 ${props.provider.name || "模型连接"}`
              : "添加模型供应商"}
          </DialogTitle>
          <DialogDescription>
            连接信息保存在本机；API Key 只存入 macOS
            钥匙串，界面保留凭据名称与引用。
          </DialogDescription>
        </DialogHeader>
        {flags.sourceUnavailable && (
          <Alert status="warning">
            <AlertTitle>配置来源尚未核对</AlertTitle>
            <AlertDescription>
              {editor.originStoreId
                ? "草稿属于原来的本机数据库，请连接原后台后核查。"
                : "当前后台未提供保存结果核查；请升级或刷新原后台后，关闭此窗口并重新打开。"}
            </AlertDescription>
          </Alert>
        )}
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void actions.save();
          }}
        >
          <Fieldset disabled={flags.fieldsetDisabled}>
            <FieldGroup>
              <ProviderConnectionFields
                presets={editor.presets}
                state={form}
                dispatch={dispatch}
                keyInput={editor.keyInput}
                busy={flags.saving || flags.fetching}
                clearError={actions.clearError}
                hasNewKey={editor.hasNewKey}
                onKeyChange={actions.keyChanged}
              />
              <ProviderCatalogSourceFields
                source={editor.catalogSource}
                disabled={
                  flags.saving || flags.fetching || flags.sourceUnavailable
                }
              />
              <ProviderModelFields
                state={form}
                dispatch={dispatch}
                activeCatalog={editor.activeCatalog}
                modelOptions={editor.modelOptions}
                modelError={editor.modelError}
                displayModel={editor.displayModel}
                fetching={flags.fetching}
                saving={flags.saving}
                canReadModels={flags.canReadModels}
                fetchModels={actions.fetchModels}
              />
            </FieldGroup>
          </Fieldset>
          <div className="mt-4 space-y-3">
            <Button
              type="button"
              variant="link"
              size="sm"
              aria-expanded={editor.showRecovery}
              aria-controls={recoveryId}
              disabled={flags.busy}
              onClick={() => editor.setShowRecovery(!editor.showRecovery)}
            >
              {editor.showRecovery
                ? "收起 API Key 恢复选项"
                : "API Key 保存与恢复"}
            </Button>
            {editor.showRecovery && (
              <div
                id={recoveryId}
                role="region"
                aria-label="API Key 保存与恢复"
                className="space-y-3"
              >
                <CredentialNamespaceRecovery
                  operations={credentials}
                  disabled={recoveryDisabled}
                />
                <CredentialDiscovery
                  operations={credentials}
                  disabled={recoveryDisabled}
                />
                {credentials.records.length > 0 && (
                  <CredentialRecovery
                    operations={credentials}
                    disabled={recoveryDisabled}
                    currentReference={form.provider.credentialRef}
                    readSecret={actions.takeSecret}
                    onAdopt={actions.adoptCredential}
                  />
                )}
              </div>
            )}
          </div>
          <ProviderWriteStatus write={write} />
          {committed && editor.viewError && (
            <Alert className="mt-4">
              <AlertTitle>保存已确认</AlertTitle>
              <AlertDescription>
                {editor.viewError}
                <Button
                  type="button"
                  variant="tertiary"
                  onClick={() => void actions.reloadSaved()}
                >
                  重新读取列表
                </Button>
              </AlertDescription>
            </Alert>
          )}
          {(shownError || pendingCredential) && (
            <Alert
              status={pendingCredential ? "warning" : "danger"}
              className="mt-4"
            >
              <AlertTitle>
                {pendingCredential
                  ? "凭据结果待核查"
                  : automatic
                    ? "连接未完成"
                    : "未能保存"}
              </AlertTitle>
              <AlertDescription>
                <p>
                  {shownError ??
                    "有凭据保存记录尚未确认或无法读取，请先核查原结果；没有自动重发保存请求。"}
                </p>
                {pendingCredential && (
                  <Button
                    type="button"
                    variant="tertiary"
                    disabled={flags.busy}
                    onClick={() => editor.setShowRecovery(true)}
                  >
                    查看待核查记录
                  </Button>
                )}
              </AlertDescription>
            </Alert>
          )}
          <DialogFooter className="mt-6">
            <Button
              type="button"
              variant="tertiary"
              disabled={flags.busy}
              onClick={props.onClose}
            >
              {write.unresolved
                ? "关闭并保留记录"
                : committed || flags.sourceUnavailable
                  ? "关闭"
                  : "取消"}
            </Button>
            <Button
              type="submit"
              variant="neutral"
              loading={flags.saving}
              disabled={flags.submitDisabled}
            >
              {automatic
                ? flags.saving
                  ? "正在识别并验证…"
                  : "连接并保存"
                : "保存连接"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
