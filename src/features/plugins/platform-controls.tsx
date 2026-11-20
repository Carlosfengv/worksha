import { useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import type {
  DirectoryPurpose,
  LogFilter,
  PlatformContribution,
  PlatformManagement,
  RotatePolicy,
} from "../../../contracts/plugins/platform/types";
import { platformPluginsApi } from "./api";
import { pluginViewHref } from "./navigation";

const directoryLabel: Record<DirectoryPurpose, string> = {
  "source-read": "会话来源目录",
  "log-sink": "日志归档目录",
};

export function PlatformControls({
  plugin,
  sinkOnly = false,
}: {
  plugin: PlatformContribution;
  sinkOnly?: boolean;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [readStatus, setReadStatus] = useState<string>();
  const [clear, setClear] = useState(false);
  const { pluginId, digest, definition, grantRevision } = plugin;
  async function perform(operation: () => Promise<unknown>) {
    if (busy) return false;
    setBusy(true);
    setError(undefined);
    try {
      await operation();
      return true;
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      return false;
    } finally {
      setBusy(false);
    }
  }
  const manage = (operation: PlatformManagement) =>
    perform(() =>
      platformPluginsApi.manage({
        pluginId,
        digest,
        expectedGrantRevision: grantRevision,
        operation,
      }),
    );
  const purposes: DirectoryPurpose[] = [];
  if (!sinkOnly && definition?.adapter?.kind === "agent-adapter")
    purposes.push("source-read");
  if (definition?.logSink) purposes.push("log-sink");
  if (plugin.diagnostic)
    return (
      <Alert status="warning">
        <AlertTitle>插件平台数据暂不可用</AlertTitle>
        <AlertDescription>{plugin.diagnostic}</AlertDescription>
      </Alert>
    );
  return (
    <section
      aria-label={sinkOnly ? "日志归档配置" : "插件数据与授权"}
      className="space-y-3"
    >
      {!sinkOnly && (
        <>
          <h4 className="text-title font-medium">插件数据与授权</h4>
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant="plain">
              {plugin.usedRows.toLocaleString()} / 10,000 条记录
            </Badge>
            <Badge variant="plain">
              {(plugin.usedBytes / 1048576).toFixed(2)} / 8 MiB
            </Badge>
            {plugin.enabled && plugin.usedRows > 0 && (
              <Button
                size="sm"
                variant="tertiary"
                onClick={() => setClear(true)}
              >
                清空私有数据
              </Button>
            )}
          </div>
          <div className="flex flex-wrap gap-2">
            {plugin.entityGrants.map((target) => (
              <Button
                key={target}
                variant="secondary"
                size="sm"
                disabled={!plugin.enabled || busy}
                onClick={() => void manage({ kind: "revoke-entity", target })}
              >
                撤销
                {
                  {
                    session: "会话",
                    project: "项目",
                    agent: "Agent",
                    provider: "供应商",
                    request: "请求",
                    account: "账户",
                  }[target]
                }
                摘要授权
              </Button>
            ))}
            {plugin.enabled &&
              (definition?.slots ?? [])
                .filter((slot) => slot.point === "nav.home-sidebar")
                .map((slot) => (
                  <Button variant="link" asChild key={slot.id}>
                    <a href={pluginViewHref(pluginId, slot.viewId)}>
                      打开{definition?.views?.[slot.viewId]?.title}
                    </a>
                  </Button>
                ))}
          </div>
        </>
      )}
      {purposes.map((purpose) => {
        const granted =
          plugin.enabled && plugin.directoryGrants.includes(purpose);
        return (
          <div key={purpose} className="flex flex-wrap items-center gap-2">
            <span className="text-body">{directoryLabel[purpose]}</span>
            <Badge variant="plain">{granted ? "已授权" : "未授权"}</Badge>
            <Button
              size="sm"
              variant="secondary"
              disabled={busy || !plugin.enabled}
              onClick={() =>
                void perform(async () => {
                  const result = await platformPluginsApi.chooseDirectory({
                    pluginId,
                    digest,
                    purpose,
                  });
                  if (
                    purpose === "source-read" &&
                    result &&
                    typeof result === "object" &&
                    "read" in result &&
                    result.read &&
                    typeof result.read === "object" &&
                    "status" in result.read
                  ) {
                    setReadStatus(
                      result.read.status === "complete"
                        ? "目录读取完成"
                        : result.read.status === "partial"
                          ? "目录已授权，本次只读取到部分数据"
                          : "已授权，目录读取待核查" +
                            ("code" in result.read
                              ? "（" + String(result.read.code) + "）"
                              : ""),
                    );
                  }
                })
              }
            >
              {granted ? "更换目录" : "选择并授权"}
            </Button>
            {granted && (
              <Button
                size="sm"
                variant="tertiary"
                disabled={busy}
                onClick={() =>
                  void manage({ kind: "revoke-directory", purpose })
                }
              >
                撤销目录授权
              </Button>
            )}
          </div>
        );
      })}
      {readStatus && (
        <p role="status" className="text-label text-fg-muted">
          {readStatus}
        </p>
      )}
      {plugin.sink && plugin.enabled && (
        <>
          <SinkReadout plugin={plugin} />
          <SinkConfiguration
            key={grantRevision}
            filter={plugin.sink.filter}
            rotate={plugin.sink.rotatePolicy}
            busy={busy}
            save={(filter, rotatePolicy) =>
              void manage({ kind: "configure-sink", filter, rotatePolicy })
            }
            flush={() => void manage({ kind: "flush-sink" })}
          />
        </>
      )}
      {error && (
        <p role="alert" className="break-words text-body text-fg-danger">
          {error}
        </p>
      )}
      {clear && (
        <Dialog
          open
          onOpenChange={(open) => {
            if (!open && !busy) setClear(false);
          }}
        >
          <DialogContent>
            <DialogTitle>清空插件私有数据</DialogTitle>
            <DialogDescription>
              此操作会删除该插件的集合记录、关联和私有键值。
            </DialogDescription>
            <DialogFooter>
              <Button
                variant="secondary"
                disabled={busy}
                onClick={() => setClear(false)}
              >
                取消
              </Button>
              <Button
                variant="destructive"
                loading={busy}
                onClick={async () => {
                  if (await manage({ kind: "clear-data" })) setClear(false);
                }}
              >
                清空数据
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}
    </section>
  );
}

export function SinkReadout({ plugin }: { plugin: PlatformContribution }) {
  const status = plugin.sink;
  return status ? (
    <div className="space-y-1 text-label text-fg-muted">
      <p>
        已归档 {status.exportedRows.toLocaleString()} 条 ·{" "}
        {(status.writtenBytes / 1048576).toFixed(2)} /{" "}
        {(status.quotaBytes / 1048576).toFixed(0)} MiB
      </p>
      {status.legacyCoverageUnknown && <p>旧游标之前的归档覆盖待核对。</p>}
      {status.lastFlushedAt && (
        <p>最近检查 {new Date(status.lastFlushedAt).toLocaleString("zh-CN")}</p>
      )}
      {status.lastError && (
        <p role="alert">归档暂未推进（{status.lastError}）</p>
      )}
    </div>
  ) : (
    <p className="text-label text-fg-muted">等待归档目录授权</p>
  );
}

function SinkConfiguration({
  filter,
  rotate,
  busy,
  save,
  flush,
}: {
  filter: LogFilter;
  rotate: RotatePolicy;
  busy: boolean;
  save: (filter: LogFilter, rotate: RotatePolicy) => void;
  flush: () => void;
}) {
  const [values, setValues] = useState(filter);
  const [rotation, setRotation] = useState(rotate);
  return (
    <form
      className="max-w-2xl space-y-3"
      onSubmit={(event) => {
        event.preventDefault();
        save(values, rotation);
      }}
    >
      <div className="grid gap-2 sm:grid-cols-2">
        {(
          [
            ["clientScope", "客户端范围（local / lan）"],
            ["outcome", "请求结果"],
            ["model", "模型名称"],
            ["agent", "Agent ID"],
          ] as const
        ).map(([key, label]) => (
          <Field key={key}>
            <FieldLabel htmlFor={`logging-${key}`}>{label}</FieldLabel>
            <Input
              id={`logging-${key}`}
              value={values[key] ?? ""}
              placeholder="全部"
              onChange={(event) =>
                setValues({ ...values, [key]: event.target.value || null })
              }
            />
          </Field>
        ))}
        <Field>
          <FieldLabel htmlFor="archive-rotation">滚动周期</FieldLabel>
          <Select
            value={rotation}
            onValueChange={(next) => {
              if (next === "daily" || next === "hourly") setRotation(next);
            }}
          >
            <SelectTrigger id="archive-rotation" />
            <SelectContent>
              <SelectItem value="daily">按天</SelectItem>
              <SelectItem value="hourly">按小时</SelectItem>
            </SelectContent>
          </Select>
        </Field>
      </div>
      <div className="flex flex-wrap gap-2">
        <Button size="sm" type="submit" disabled={busy}>
          保存归档规则
        </Button>
        <Button
          size="sm"
          type="button"
          variant="secondary"
          disabled={busy}
          onClick={flush}
        >
          立即归档
        </Button>
      </div>
    </form>
  );
}
