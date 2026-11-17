import { useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { createReadResource } from "@/shared/lib/read-resource";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { accountsApi } from "../api";
import type { ClaudeStatusLinePreview } from "../types";

const statusLineResource = createReadResource(() =>
  accountsApi.claudeStatusLine({ kind: "status" }),
);

export function ClaudeStatusLineControl({ onApply }: { onApply: () => void }) {
  const status = useSharedRead(statusLineResource);
  const [preview, setPreview] = useState<ClaudeStatusLinePreview | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const config = status.data?.config;
  const enabled = config?.state === "installed" || config?.state === "replaced";
  const reviewId = preview?.reviewId ?? config?.pending;

  const prepare = async () => {
    if (!config || busy) return;
    setBusy(true);
    setError("");
    try {
      setPreview(
        await accountsApi.claudeStatusLine({
          kind: "preview",
          enabled: !enabled,
          expectedRevision: config.revision,
        }),
      );
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      void status.invalidate();
    } finally {
      setBusy(false);
    }
  };

  const commit = async (kind: "apply" | "cancel") => {
    if (!reviewId || busy) return;
    setBusy(true);
    setError("");
    try {
      await accountsApi.claudeStatusLine({ kind, reviewId });
      setPreview(null);
      if (kind === "apply") onApply();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      void status.invalidate();
      setBusy(false);
    }
  };

  return (
    <section className="space-y-3" aria-label="Claude Code 状态栏">
      {config && (
        <p role="status" className="text-label text-fg-muted">
          {config.state === "installed"
            ? status.data?.received
              ? "已配置 · 已收到会话快照"
              : "已配置 · 等待会话额度"
            : config.state === "custom"
              ? "已配置自定义状态栏，可选择下方的独立入口"
              : config.state === "replaced"
                ? "状态栏已在外部修改，受管入口已停止采集"
                : "用户级状态栏尚未接入"}
        </p>
      )}
      {!config && status.loading && (
        <p role="status" className="text-label text-fg-muted">
          正在读取状态栏配置…
        </p>
      )}
      {(error || status.error) && (
        <Alert>
          <AlertTitle>状态栏配置未更新</AlertTitle>
          <AlertDescription>{error || status.error}</AlertDescription>
        </Alert>
      )}
      {preview && (
        <div className="space-y-2 rounded-xl border border-border p-3 text-label">
          <p className="break-all">{preview.configPath}</p>
          {preview.statusLine ? (
            <>
              <p>将写入以下状态栏命令。项目或组织设置可能覆盖此配置。</p>
              <p className="break-all font-mono">
                {preview.statusLine.command}
              </p>
            </>
          ) : (
            <p>将关闭受管额度采集，并保留其他配置和用户设置的状态栏。</p>
          )}
        </div>
      )}
      <div className="flex flex-wrap gap-2">
        {reviewId ? (
          <>
            <Button
              size="sm"
              variant="tertiary"
              disabled={busy}
              onClick={() => void commit("cancel")}
            >
              撤销预览
            </Button>
            {preview && (
              <Button
                size="sm"
                loading={busy}
                onClick={() => void commit("apply")}
              >
                确认{preview.enabled ? "开启" : "关闭"}
              </Button>
            )}
          </>
        ) : config && config.state !== "custom" ? (
          <Button
            size="sm"
            variant="neutral"
            loading={busy}
            onClick={() => void prepare()}
          >
            预览{enabled ? "关闭" : "开启"}状态栏采集
          </Button>
        ) : status.error ? (
          <Button size="sm" variant="tertiary" onClick={status.reload}>
            重新读取
          </Button>
        ) : null}
      </div>
    </section>
  );
}
