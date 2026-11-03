import { useEffect, useRef, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Field, FieldLabel, FieldDescription } from "@/components/ui/field";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { settingsApi } from "../api";
import type { Settings, SettingsWriteProtocol } from "../types";
import type { RetentionContentInventory } from "../retention-content-inventory";
import { settingsProtocol } from "../save-result";
import {
  GatewayRetentionDetails,
  ActivityRetentionDetails,
  ToolRetentionDetails,
  UnverifiedRetentionSource,
  retentionDate,
} from "./retention-source-details";

type State =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "error"; message: string }
  | { kind: "ready"; value: RetentionContentInventory };
function ScopedPreview({
  storeId,
  revision,
  initialDays,
}: {
  storeId: string;
  revision: number;
  initialDays: Settings["retentionDays"];
}) {
  const [days, setDays] = useState(initialDays);
  const [state, setState] = useState<State>({ kind: "idle" });
  const [expired, setExpired] = useState(false);
  const active = useRef(false),
    locked = useRef(false);
  useEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
    };
  }, []);
  const validUntil =
    state.kind === "ready" ? state.value.validUntil : undefined;
  useEffect(() => {
    if (!validUntil) return;
    const timer = window.setTimeout(
      () => setExpired(true),
      Math.max(0, Date.parse(validUntil) - Date.now()),
    );
    return () => window.clearTimeout(timer);
  }, [validUntil]);
  const run = async () => {
    if (locked.current) return;
    locked.current = true;
    setState({ kind: "loading" });
    setExpired(false);
    try {
      const value = await settingsApi.previewRetentionContentInventory({
        storeId,
        expectedSettingsRevision: revision,
        days,
      });
      if (active.current) {
        setState({ kind: "ready", value });
        setExpired(Date.parse(value.validUntil) <= Date.now());
      }
    } catch (reason) {
      if (active.current)
        setState({
          kind: "error",
          message:
            reason instanceof Error
              ? reason.message
              : "留存预览未完成，请重试。",
        });
    } finally {
      locked.current = false;
    }
  };
  const result = state.kind === "ready" ? state.value : null;
  const g = result?.gateway.status === "checked" ? result.gateway.value : null;
  const t = result?.tools.status === "checked" ? result.tools.value : null;
  const incomplete =
    result &&
    (!g ||
      !t ||
      !g.classificationComplete ||
      !t.classificationComplete ||
      t.capacityLimit === null ||
      result.writerIssue ||
      result.pendingWrites > 0 ||
      Object.values(result.activityCaches).some(
        (source) => source.status !== "checked" || source.value.unknownTime > 0,
      ));
  return (
    <>
      <Field>
        <FieldLabel>预览保留期</FieldLabel>
        <Select
          value={String(days)}
          disabled={state.kind === "loading"}
          onValueChange={(value) => {
            if (value === "7" || value === "30" || value === "90") {
              setDays(Number(value) as Settings["retentionDays"]);
              setState({ kind: "idle" });
              setExpired(false);
            }
          }}
        >
          <SelectTrigger aria-label="预览保留期" />
          <SelectContent>
            <SelectItem value="7">7 天</SelectItem>
            <SelectItem value="30">30 天</SelectItem>
            <SelectItem value="90">90 天</SelectItem>
          </SelectContent>
        </Select>
        <FieldDescription>
          仅用于本次估算，不更改已保存的偏好。
        </FieldDescription>
      </Field>
      <Button
        variant="neutral"
        loading={state.kind === "loading"}
        onClick={() => void run()}
      >
        预览影响
      </Button>
      <div
        aria-live="polite"
        aria-busy={state.kind === "loading"}
        className="space-y-4"
      >
        {state.kind === "idle" && (
          <p className="text-body text-fg-muted">尚未检查影响数量。</p>
        )}
        {state.kind === "loading" && (
          <p role="status" className="text-body text-fg-muted">
            正在检查网关与代理工具记录…
          </p>
        )}
        {state.kind === "error" && (
          <Alert status="warning">
            <AlertTitle>预览未完成</AlertTitle>
            <AlertDescription>{state.message}</AlertDescription>
          </Alert>
        )}
        {result && (
          <>
            <p className="text-body text-fg-muted">
              检查时间：{retentionDate(result.capturedAt)}；预计截止时间：
              {retentionDate(result.cutoff)}（严格早于）。
            </p>
            {expired ? (
              <Alert status="warning">
                <AlertTitle>预览已过期</AlertTitle>
                <AlertDescription>
                  以下为上次检查结果，请重新预览。记录仍未清理。
                </AlertDescription>
              </Alert>
            ) : (
              <p className="text-body text-fg-muted">
                此结果最多保留 5 分钟；后续写入可能改变数量。
              </p>
            )}
            <p className="text-body text-fg-muted">
              各来源分别检查，读取时间可能不同。冻结查询、未完成导出及其他派生副本尚未核查，这些数量仍不能用于确认清理。
            </p>
            <section aria-label="网关请求预览" className="space-y-3">
              <h3 className="text-body font-medium">网关请求</h3>
              {result.gateway.status === "checked" ? (
                <GatewayRetentionDetails source={result.gateway} />
              ) : (
                <UnverifiedRetentionSource reason={result.gateway.reason} />
              )}
            </section>
            <section aria-label="代理工具预览" className="space-y-3">
              <h3 className="text-body font-medium">本产品代理工具</h3>
              {result.tools.status === "checked" ? (
                <ToolRetentionDetails source={result.tools} />
              ) : (
                <UnverifiedRetentionSource reason={result.tools.reason} />
              )}
            </section>
            <p className="text-body text-fg-muted">
              当前待写网关更新：{result.pendingWrites}
              。待写数是另一个时点的状态，不代表所有写入已经排空。
            </p>
            {(
              [
                ["gateway", "网关活动摘要缓存"],
                ["native", "原生活动摘要缓存"],
              ] as const
            ).map(([kind, title]) => {
              const source = result.activityCaches[kind];
              return (
                <section key={kind} aria-label={title} className="space-y-3">
                  <h3 className="text-body font-medium">{title}</h3>
                  {source.status === "checked" ? (
                    <ActivityRetentionDetails source={source} />
                  ) : (
                    <UnverifiedRetentionSource reason={source.reason} />
                  )}
                </section>
              );
            })}
            <p className="text-body text-fg-muted">
              原生活动摘要是本产品的缓存副本；原生会话文件仍不在清理范围内。摘要写入与缓存容量淘汰可能改变数量。
            </p>
            {incomplete && (
              <Alert status="warning">
                <AlertTitle>覆盖范围尚不完整</AlertTitle>
                <AlertDescription>
                  存在待核查来源、待写更新或写入异常，当前数量不能用于确认清理。
                </AlertDescription>
              </Alert>
            )}
          </>
        )}
      </div>
    </>
  );
}
export function RetentionPreviewPanel({
  protocol,
  revision,
  days,
}: {
  protocol?: SettingsWriteProtocol;
  revision?: number;
  days?: Settings["retentionDays"];
}) {
  const supported =
    settingsProtocol(protocol) &&
    Number.isSafeInteger(revision) &&
    (revision ?? -1) >= 0 &&
    days !== undefined;
  return (
    <section aria-label="留存影响预览" className="space-y-4">
      <div>
        <h2 className="text-title font-medium">留存影响预览</h2>
        <p className="mt-1 text-body text-fg-muted">
          自动清理与额外内容归档尚未启用。旧安装须在功能开放后明确确认，现有偏好不会直接启用清理；允许保留的脱敏汇总持续保留。
        </p>
      </div>
      <p className="text-body text-fg-muted">
        检查本产品已入库的网关请求、当前保存的代理工具记录及活动摘要缓存。原生会话不在清理范围内。预览不会清理记录，确认入口尚未开放。
      </p>
      {supported ? (
        <ScopedPreview
          key={`${protocol!.storeId}:${revision}`}
          storeId={protocol!.storeId}
          revision={revision!}
          initialDays={days!}
        />
      ) : (
        <p className="text-body text-fg-muted">
          本机设置身份尚未确认，暂不能预览影响数量。
        </p>
      )}
    </section>
  );
}
