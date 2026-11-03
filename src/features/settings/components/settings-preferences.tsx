import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { DesktopError } from "@/shared/lib/desktop-command";
import { settingsApi, SettingsPreflightError } from "../api";
import type {
  Settings,
  ScopedSettingsSaved,
  SettingsUpdate,
  SettingsWriteProtocol,
} from "../types";
import { freezeSettingsRequest, settingsProtocol } from "../save-result";
import {
  readSettingsRecovery,
  rememberSettingsOperation,
  requireSettingsRecovery,
  forgetSettingsOperation,
  SETTINGS_OPERATIONS_CHANGED,
} from "../pending-settings";
import {
  SettingsOperationRecovery,
  SettingsRequestSummary,
} from "./settings-operation-recovery";

type Version = {
  value: Settings;
  revision: number | undefined;
  storeId: string | undefined;
};
type Operation =
  | { kind: "idle" }
  | { kind: "saving" | "checking"; request: SettingsUpdate }
  | { kind: "unknown"; request: SettingsUpdate; message: string }
  | { kind: "failed"; message: string }
  | { kind: "saved"; storeId: string; message: string; cleanup?: string };
const same = (a: Settings, b: Settings) =>
  a.captureContent === b.captureContent && a.retentionDays === b.retentionDays;
const message = (reason: unknown) =>
  reason instanceof Error ? reason.message : String(reason);

export function SettingsPreferences({
  value,
  revision,
  protocol,
  refresh,
}: {
  value?: Settings;
  revision: number | undefined;
  protocol?: SettingsWriteProtocol;
  refresh: () => Promise<void>;
}) {
  const [draft, setDraft] = useState<{ base: Version; value: Settings } | null>(
    null,
  );
  const [confirmed, setConfirmed] = useState<ScopedSettingsSaved | null>(null);
  const [operation, setOperation] = useState<Operation>({ kind: "idle" });
  const [recovery, setRecovery] = useState(readSettingsRecovery);
  const generation = useRef(0);
  const active = useRef(false);
  const locked = useRef(false);
  const storeId = settingsProtocol(protocol) ? protocol.storeId : undefined;
  const visibleStore = useRef(storeId);
  useLayoutEffect(() => {
    visibleStore.current = storeId;
  }, [storeId]);
  useEffect(() => {
    active.current = true;
    const reload = () => setRecovery(readSettingsRecovery());
    window.addEventListener(SETTINGS_OPERATIONS_CHANGED, reload);
    window.addEventListener("storage", reload);
    return () => {
      active.current = false;
      generation.current += 1;
      window.removeEventListener(SETTINGS_OPERATIONS_CHANGED, reload);
      window.removeEventListener("storage", reload);
    };
  }, []);
  const current: Version | undefined = !value
    ? undefined
    : confirmed &&
        confirmed.storeId === storeId &&
        confirmed.revision > (revision ?? -1)
      ? confirmed
      : { value, revision, storeId };
  const shown = draft?.value ?? current?.value;
  const base = draft?.base ?? current;
  const dirty = !!draft && !same(draft.value, draft.base.value);
  const changedStore = dirty && base?.storeId !== current?.storeId;
  const conflict =
    dirty && (changedStore || base?.revision !== current?.revision);
  const supported =
    !!storeId &&
    current?.revision !== undefined &&
    Number.isSafeInteger(current.revision) &&
    current.revision >= 0;
  const busy = operation.kind === "saving" || operation.kind === "checking";
  const uncertain = operation.kind === "unknown";
  const pendingForStore = recovery.requests.some((r) => r.storeId === storeId);
  const recoveryBlocked =
    !recovery.available ||
    recovery.invalid > 0 ||
    recovery.atCapacity ||
    pendingForStore;
  const change = (patch: Partial<Settings>) => {
    if (!shown || !base) return;
    const next = { ...shown, ...patch };
    setDraft(same(next, base.value) ? null : { base, value: next });
    setOperation({ kind: "idle" });
  };
  const isCurrent = (token: number) =>
    active.current && generation.current === token;
  const accept = (
    result: ScopedSettingsSaved,
    request: SettingsUpdate,
    token: number,
  ) => {
    if (!isCurrent(token)) return;
    let cleanup = "";
    try {
      forgetSettingsOperation(request);
    } catch (reason) {
      cleanup = message(reason);
    }
    if (result.storeId === visibleStore.current)
      setConfirmed((previous) =>
        previous?.storeId === result.storeId &&
        previous.revision > result.revision
          ? previous
          : result,
      );
    setDraft((previous) =>
      previous?.base.storeId === result.storeId ? null : previous,
    );
    setOperation({
      kind: "saved",
      storeId: result.storeId,
      message: "偏好已保存到本机。内容归档与按天自动到期尚未启用。",
      cleanup,
    });
    void refresh().catch(() => {
      /* The durable operation receipt already confirms this save. */
    });
  };
  const verify = async (request: SettingsUpdate, token: number) => {
    if (isCurrent(token)) setOperation({ kind: "checking", request });
    try {
      accept(
        await settingsApi.scopedSettingsOperation(request),
        request,
        token,
      );
    } catch (reason) {
      if (isCurrent(token))
        setOperation({
          kind: "unknown",
          request,
          message: `尚未确认保存结果，原请求已保留。${message(reason)}`,
        });
    }
  };
  const save = async (retry?: SettingsUpdate) => {
    if (retry && retry.storeId !== storeId) {
      if (!locked.current)
        setOperation({
          kind: "unknown",
          request: retry,
          message:
            "原保存属于另一个本机数据库，请连接原后台后核查；未发送新的保存请求。",
        });
      return;
    }
    if (
      locked.current ||
      (!retry &&
        (!dirty ||
          conflict ||
          !supported ||
          uncertain ||
          recoveryBlocked ||
          !base ||
          !shown))
    )
      return;
    let request: SettingsUpdate;
    try {
      request = freezeSettingsRequest(
        retry ?? {
          storeId: base!.storeId!,
          value: {
            captureContent: shown!.captureContent,
            retentionDays: shown!.retentionDays,
          },
          expectedRevision: base!.revision!,
          operationId: crypto.randomUUID(),
        },
      );
      if (retry) requireSettingsRecovery(request);
      else rememberSettingsOperation(request);
    } catch (reason) {
      setRecovery(readSettingsRecovery());
      setOperation(
        retry
          ? { kind: "unknown", request: retry, message: message(reason) }
          : { kind: "failed", message: message(reason) },
      );
      return;
    }
    locked.current = true;
    const token = generation.current;
    setOperation({ kind: "saving", request });
    try {
      accept(await settingsApi.saveSettings(request), request, token);
    } catch (reason) {
      if (!isCurrent(token)) return;
      if (
        !retry &&
        reason instanceof DesktopError &&
        !reason.outcomeUnknown &&
        (reason instanceof SettingsPreflightError ||
          [
            "conflict",
            "operation_conflict",
            "invalid_input",
            "operation_store_changed",
            "write_protocol_unavailable",
            "settings_protocol_unavailable",
          ].includes(reason.code))
      ) {
        let cleanup = "";
        try {
          forgetSettingsOperation(request);
        } catch {
          cleanup = "原请求记录仍保留，请先核查后再开始新的保存。";
        }
        setOperation({ kind: "failed", message: message(reason) + cleanup });
        void refresh().catch(() => {});
      } else {
        await verify(request, token);
      }
    } finally {
      locked.current = false;
    }
  };
  const check = async (request: SettingsUpdate) => {
    if (locked.current) return;
    locked.current = true;
    try {
      await verify(request, generation.current);
    } finally {
      locked.current = false;
    }
  };
  const recoveryView = (
    <SettingsOperationRecovery
      pending={recovery}
      activeId={
        value && "request" in operation
          ? operation.request.operationId
          : undefined
      }
      storeId={supported ? storeId : undefined}
      busy={busy}
      notice={
        !value && "message" in operation
          ? operation.message +
            (operation.kind === "saved" ? (operation.cleanup ?? "") : "")
          : undefined
      }
      check={(request) => void check(request)}
      retry={(request) => void save(request)}
    />
  );
  if (!value || !shown || !base) return recoveryView;
  return (
    <section className="space-y-4" aria-label="内容与留存偏好">
      {recoveryView}
      <div>
        <h2 className="text-title font-medium">内容与留存</h2>
        <p className="mt-1 text-body text-fg-muted">
          内容归档与按天自动到期尚未启用，此处仅保存偏好。现有记录与原生会话不会因选择天数被删除。
        </p>
      </div>
      {!supported && (
        <Alert status="warning">
          <AlertTitle>当前后台暂不支持安全保存设置</AlertTitle>
          <AlertDescription>
            请正常退出旧服务后重新打开当前应用，现有设置继续保留。
          </AlertDescription>
        </Alert>
      )}
      {conflict && (
        <Alert status="warning">
          <AlertTitle>
            {changedStore ? "设置所属后台已变化" : "设置已有新版本"}
          </AlertTitle>
          <AlertDescription>
            {changedStore
              ? "原后台草稿已保留，未提交到当前数据库。请返回原后台，或撤销修改后核对当前设置。"
              : "你的草稿已保留。请撤销修改，核对最新设置后重新编辑。"}
          </AlertDescription>
        </Alert>
      )}
      <FieldGroup>
        <Field>
          <Switch
            label="额外保存消息与工具内容"
            checked={shown.captureContent}
            disabled={
              busy || uncertain || !supported || changedStore || pendingForStore
            }
            onCheckedChange={(captureContent) => change({ captureContent })}
          />
          <FieldDescription>
            此偏好尚未接入额外内容归档；授权后的原生会话仍按需读取。
          </FieldDescription>
        </Field>
        <Field>
          <FieldLabel>记录保留期</FieldLabel>
          <Select
            value={String(shown.retentionDays)}
            disabled={
              busy || uncertain || !supported || changedStore || pendingForStore
            }
            onValueChange={(days) => {
              if (days === "7" || days === "30" || days === "90")
                change({
                  retentionDays: Number(days) as Settings["retentionDays"],
                });
            }}
          >
            <SelectTrigger aria-label="记录保留期" />
            <SelectContent>
              <SelectItem value="7">7 天</SelectItem>
              <SelectItem value="30">30 天</SelectItem>
              <SelectItem value="90">90 天</SelectItem>
            </SelectContent>
          </Select>
          <FieldDescription>自动到期处理尚未启用。</FieldDescription>
        </Field>
      </FieldGroup>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant="neutral"
          disabled={
            !dirty || conflict || !supported || uncertain || recoveryBlocked
          }
          loading={busy}
          onClick={() => void save()}
        >
          保存偏好
        </Button>
        <Button
          variant="tertiary"
          disabled={!dirty || busy || uncertain}
          onClick={() => {
            setDraft(null);
            setOperation({ kind: "idle" });
          }}
        >
          撤销修改
        </Button>
        {dirty && (
          <span className="text-label text-fg-muted">有未保存的修改</span>
        )}
      </div>
      {operation.kind === "checking" && (
        <p role="status" className="text-label text-fg-muted">
          正在核查保存结果…
        </p>
      )}
      {(operation.kind === "unknown" ||
        operation.kind === "failed" ||
        operation.kind === "saved") && (
        <Alert
          role={operation.kind === "saved" ? "status" : "alert"}
          status={
            operation.kind === "failed"
              ? "danger"
              : operation.kind === "unknown"
                ? "warning"
                : "default"
          }
        >
          <AlertTitle>
            {operation.kind === "saved"
              ? "偏好已保存"
              : operation.kind === "unknown"
                ? "保存结果待确认"
                : "未能保存"}
          </AlertTitle>
          <AlertDescription>
            {operation.kind === "saved" && operation.storeId !== storeId
              ? "原后台的设置已确认保存；此结果不替换当前后台设置。"
              : operation.message}
            {operation.kind === "saved" && operation.cleanup}
            {operation.kind === "unknown" && (
              <SettingsRequestSummary
                request={operation.request}
                storeId={storeId}
              />
            )}
          </AlertDescription>
          {operation.kind === "unknown" && (
            <div className="mt-3 flex flex-wrap gap-2">
              <Button
                variant="tertiary"
                size="sm"
                onClick={() => void check(operation.request)}
              >
                核查保存结果
              </Button>
              <Button
                variant="tertiary"
                size="sm"
                onClick={() => void save(operation.request)}
              >
                重试同一保存
              </Button>
            </div>
          )}
        </Alert>
      )}
    </section>
  );
}
