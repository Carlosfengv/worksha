import { useEffect, useId, useRef, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  Stepper,
  StepperList,
  StepperItem,
  StepperTitle,
  StepperTrigger,
} from "@/components/ui/stepper";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
  Fieldset,
} from "@/components/ui/field";
import { isLoopbackEndpoint } from "../catalog";
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

export const providerWizardFormId = "new-provider-wizard";

export function ProviderEditor(
  props: ProviderEditorProps & { guidedCreation?: boolean },
) {
  const editor = useProviderEditor(props);
  const [step, setStep] = useState("endpoint");
  const guided = props.guidedCreation && !props.provider.configured;
  const steps = [
    ["endpoint", "端点与协议"],
    ["credentials", "凭据与 16-Token 探活"],
    ["models", "模型目录与快捷建路由"],
  ];
  const titleId = useId();
  const recoveryId = useId();
  const validationId = useId();
  const feedbackRef = useRef<HTMLDivElement>(null);
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
  const endpointReady = form.vendorChosen && !!form.provider.endpoint.trim();
  const credentialReady =
    editor.hasNewKey ||
    !!form.provider.credentialRef ||
    isLoopbackEndpoint(form.provider.endpoint);
  const availableStep = (next: string) =>
    next === "endpoint" ||
    (endpointReady && (next === "credentials" || credentialReady));
  const moveTo = (next: string) => {
    if (flags.busy || !availableStep(next)) return;
    setStep(next);
    if (next === "models" && !editor.activeCatalog && flags.canReadModels)
      void actions.fetchModels();
  };
  const makeService =
    props.canCreateDefaultService && editor.createDefaultService;
  const recoveryDisabled =
    flags.saving ||
    flags.fetching ||
    flags.probing ||
    flags.presetResolving ||
    flags.sourceUnavailable;
  const pendingCredential = flags.credentialBlocked;
  const refreshError = committed ? editor.viewError : null;
  useEffect(() => {
    if (shownError || pendingCredential || write.unresolved || refreshError)
      feedbackRef.current?.scrollIntoView({ block: "nearest" });
  }, [shownError, pendingCredential, write.unresolved, refreshError]);
  const formContent = (
    <form
      id={guided ? providerWizardFormId : undefined}
      className="flex min-h-0 flex-col"
      onReset={(event) => {
        event.preventDefault();
        if (!flags.busy) props.onClose();
      }}
      onSubmit={(event) => {
        event.preventDefault();
        if (!guided || step === "models") void actions.save();
      }}
    >
      {guided && (
        <Stepper
          value={step}
          onValueChange={moveTo}
          disabled={flags.busy}
          activationMode="manual"
          className="pb-3"
        >
          <StepperList aria-label="供应商设置步骤">
            {steps.map(([value, label]) => (
              <StepperItem
                key={value}
                value={value}
                disabled={!availableStep(value)}
              >
                <StepperTrigger type="button">
                  <StepperTitle>{label}</StepperTitle>
                </StepperTrigger>
              </StepperItem>
            ))}
          </StepperList>
        </Stepper>
      )}
      <div
        className={
          guided ? "min-w-0" : "-mx-1 min-h-0 overflow-y-auto px-1 pb-1"
        }
      >
        {flags.sourceUnavailable && (
          <Alert status="warning" className="mb-3">
            <AlertTitle>配置来源尚未核对</AlertTitle>
            <AlertDescription>
              {editor.originStoreId
                ? "草稿属于原来的本机数据库，请连接原后台后核查。"
                : "当前后台无法核查保存结果。更新或重新连接后再打开此表单。"}
            </AlertDescription>
          </Alert>
        )}
        <Fieldset disabled={flags.fieldsetDisabled}>
          <FieldGroup>
            <ProviderConnectionFields
              step={guided ? step : undefined}
              presets={editor.presets}
              state={form}
              dispatch={dispatch}
              keyInput={editor.keyInput}
              busy={flags.saving || flags.fetching || flags.probing}
              clearError={actions.clearError}
              hasNewKey={editor.hasNewKey}
              onKeyChange={actions.keyChanged}
            />
            {guided && step === "credentials" && (
              <Field>
                <FieldLabel>探活模型 ID</FieldLabel>
                <Input
                  value={form.provider.model}
                  maxLength={200}
                  onChange={(event) =>
                    dispatch({ type: "model", value: event.target.value })
                  }
                  placeholder="知道模型 ID 时可先验证"
                />
                <FieldDescription>
                  向所选模型发送一次最多 16 个输出 Token 的短请求，可能产生 API
                  费用。未知模型 ID 时，可先继续获取目录。
                </FieldDescription>
                <div className="flex flex-wrap items-center gap-3">
                  <Button
                    type="button"
                    variant="secondary"
                    loading={flags.probing}
                    disabled={!credentialReady || !form.provider.model.trim()}
                    onClick={() => void actions.probe()}
                  >
                    验证连接
                  </Button>
                  {editor.probeVerified && (
                    <span role="status">短请求验证通过</span>
                  )}
                </div>
              </Field>
            )}
            <div className="space-y-3" hidden={guided && step !== "models"}>
              <ProviderCatalogSourceFields
                source={editor.catalogSource}
                disabled={
                  flags.saving || flags.fetching || flags.sourceUnavailable
                }
              />
              <ProviderModelFields
                guided={guided}
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
            </div>
          </FieldGroup>
        </Fieldset>
        <div
          className="mt-4 space-y-3"
          hidden={guided && step !== "credentials"}
        >
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
        <div ref={feedbackRef}>
          <ProviderWriteStatus write={write} />
          {refreshError && (
            <Alert className="mt-4">
              <AlertTitle>保存已确认</AlertTitle>
              <AlertDescription>
                {refreshError}
                <Button
                  type="button"
                  variant="tertiary"
                  onClick={() => {
                    if (committed) void actions.reloadSaved(committed);
                  }}
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
                  : flags.probing || step === "credentials" || automatic
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
                    onClick={() => {
                      if (guided) setStep("credentials");
                      editor.setShowRecovery(true);
                    }}
                  >
                    查看待核查记录
                  </Button>
                )}
              </AlertDescription>
            </Alert>
          )}
        </div>
      </div>
      {automatic && (
        <p id={validationId} className="mt-3 text-label text-fg-muted">
          验证最多发送 6 次短模型请求，每次最多 16 个输出 Token，可能产生 API
          费用。
        </p>
      )}
      {props.canCreateDefaultService && guided && step === "models" && (
        <label className="mt-3 flex items-center gap-2 text-body font-medium">
          <Checkbox
            aria-label="同时生成同名默认模型服务"
            checked={editor.createDefaultService}
            onCheckedChange={editor.setCreateDefaultService}
            disabled={flags.busy || !!committed}
          />
          同时生成同名默认模型服务
        </label>
      )}
      <DialogFooter className="mt-3 shrink-0">
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
        {guided && step !== "endpoint" && (
          <Button
            type="button"
            variant="secondary"
            disabled={flags.busy}
            onClick={() =>
              moveTo(step === "models" ? "credentials" : "endpoint")
            }
          >
            上一步
          </Button>
        )}
        {guided && step !== "models" ? (
          <Button
            key="continue"
            type="button"
            disabled={
              flags.busy ||
              !availableStep(step === "endpoint" ? "credentials" : "models")
            }
            onClick={(event) => {
              event.preventDefault();
              moveTo(step === "endpoint" ? "credentials" : "models");
            }}
          >
            {step === "endpoint" ? "下一步" : "继续并获取模型"}
          </Button>
        ) : (
          <Button
            key="save"
            type="submit"
            variant="neutral"
            aria-describedby={automatic ? validationId : undefined}
            loading={flags.saving}
            disabled={flags.submitDisabled}
          >
            {guided
              ? makeService
                ? "保存并前往绑定 Agent"
                : "保存并管理模型"
              : automatic
                ? flags.saving
                  ? "正在识别并验证…"
                  : "连接并保存"
                : "保存连接"}
          </Button>
        )}
      </DialogFooter>
    </form>
  );
  if (guided)
    return (
      <section
        aria-labelledby={titleId}
        className="space-y-4 rounded-xl border border-border bg-surface p-4"
      >
        <h3 id={titleId} className="text-title font-medium">
          添加模型供应商
        </h3>
        {formContent}
      </section>
    );
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !flags.busy) props.onClose();
      }}
    >
      <DialogContent
        size="lg"
        className="flex max-h-[calc(100dvh-2rem)] flex-col overflow-hidden"
      >
        <DialogHeader className="shrink-0">
          <DialogTitle>
            {props.provider.configured
              ? `配置 ${props.provider.name || "模型连接"}`
              : "添加模型供应商"}
          </DialogTitle>
          <DialogDescription>配置连接与默认模型。</DialogDescription>
        </DialogHeader>
        {formContent}
      </DialogContent>
    </Dialog>
  );
}
