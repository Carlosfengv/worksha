import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import { domainApi } from "./api";
import {
  domainDirectoryResource,
  sourceConsentReviewResource,
} from "./queries";

export function DomainConsentControls({
  pluginId,
  digest,
  sourceId = pluginId + ":source",
  fullFacts = false,
  onChanged,
}: {
  pluginId: string;
  digest: string;
  sourceId?: string;
  fullFacts?: boolean;
  onChanged: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [persist, setPersist] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const review = useSharedQuery(
    sourceConsentReviewResource,
    { pluginId, digest },
    open,
  );
  const current =
    !review.loading && !review.error && !error ? review.data : null;
  const persistMetadata = persist ?? current?.persistMetadata ?? false;
  const remembersSource =
    fullFacts && sourceId === "net.worksha.dsh-desktop:desktop";
  const save = async (enabled: boolean) => {
    if (busy || !current || error) return;
    setBusy(true);
    setError(null);
    try {
      await domainApi.consent({
        pluginId,
        digest,
        enabled,
        persistMetadata: fullFacts && !enabled ? false : persistMetadata,
        reviewRef: current.reviewRef,
      });
      await Promise.all([
        review.invalidate(),
        domainDirectoryResource.invalidate(),
      ]);
      onChanged();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="space-y-3" aria-label="统一会话展示授权">
      <Button
        variant="tertiary"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        统一会话展示
      </Button>
      {open && (
        <div className="space-y-3 rounded-xl border border-border p-4">
          <p className="text-body">
            {fullFacts
              ? "将已读取的项目、会话、响应、活动及 MCP 证据加入统一查询。"
              : "将已授权来源的会话标题、时间和用量加入统一查询。"}
            {remembersSource
              ? "桥接更新后会自动读取所选文件。"
              : "不会扩大插件读取范围，也不会自动刷新来源。"}
          </p>
          <p className="text-label text-fg-muted">
            {remembersSource
              ? "Worksha 会记住所选文件和展示授权，重开时核对后自动恢复。替换来源文件后需要重新授权。"
              : fullFacts
                ? "默认仅保留本次运行的元数据；保存的元数据重开后标记过期，需重新启用、选择资源并授权展示后才能读取。"
                : "默认仅保留本次运行的元数据；重开后需手动刷新。可选择保存元数据，重开时旧记录会标记过期。"}
            正文始终按需读取，不另存副本。
          </p>
          {review.loading && (
            <p role="status" className="text-label text-fg-muted">
              正在核查展示范围…
            </p>
          )}
          {current?.resourceLabel && (
            <p className="break-all text-label text-fg-muted">
              来源文件：{current.resourceLabel}
            </p>
          )}
          {current?.unifiedAuthorized && (
            <p role="status" className="text-label">
              已加入统一展示 ·{" "}
              {remembersSource
                ? "重开时恢复"
                : current.persistMetadata
                  ? "保存元数据"
                  : "仅本次运行"}
            </p>
          )}
          <label className="flex items-center gap-2 text-label">
            <Checkbox
              checked={persistMetadata}
              disabled={busy || !current || !!error}
              onCheckedChange={(value) => setPersist(value === true)}
            />
            {remembersSource
              ? "保存会话元数据"
              : fullFacts
                ? "保存元数据（重开后需要重新授权）"
                : "保存元数据供重开读取"}
          </label>
          {current && !current.sourceAuthorized && (
            <p className="text-label text-fg-muted">请先选择并授权来源文件。</p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button
              disabled={
                busy || !current || !!error || !current.sourceAuthorized
              }
              onClick={() => void save(true)}
            >
              {current?.unifiedAuthorized ? "更新展示授权" : "确认加入统一展示"}
            </Button>
            {current?.unifiedAuthorized && (
              <Button
                variant="tertiary"
                disabled={busy || !!error}
                onClick={() => void save(false)}
              >
                停止统一展示
              </Button>
            )}
            {(error || review.error) && (
              <Button
                variant="tertiary"
                disabled={busy || review.loading}
                onClick={() => {
                  setError(null);
                  setPersist(null);
                  void review.invalidate();
                }}
              >
                重新核查展示范围
              </Button>
            )}
            <Button asChild variant="tertiary">
              <a
                href={`#/sessions?scope=unified&source=${encodeURIComponent(sourceId)}`}
              >
                查看统一会话
              </a>
            </Button>
          </div>
          <p className="text-label text-fg-subtle">
            停止统一展示会使对应统一查询失效，保留原来源授权与扩展视图。
          </p>
          {(error || review.error) && (
            <p role="alert" className="text-label text-fg-muted">
              {error ?? review.error}
            </p>
          )}
        </div>
      )}
    </section>
  );
}
