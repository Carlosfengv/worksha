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
  const [watch, setWatch] = useState<boolean | null>(null);
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
  const persistWatch = watch ?? current?.persistWatch ?? false;
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
        persistWatch:
          enabled && current.canPersistWatch === true && persistWatch,
        reviewRef: current.reviewRef,
      });
      setPersist(null);
      setWatch(null);
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
        onClick={() => {
          setPersist(null);
          setWatch(null);
          setOpen(!open);
        }}
      >
        统一会话展示
      </Button>
      {open && (
        <div className="space-y-3 rounded-xl border border-border p-4">
          <p className="text-body">
            {fullFacts
              ? "将已读取的项目、会话、响应、活动及 MCP 证据加入统一查询。"
              : "将已授权来源的会话标题、时间和用量加入统一查询。"}
          </p>
          <p className="text-label text-fg-muted">
            {current?.canPersistWatch &&
              "持续监听只读取选定的文件，替换文件后需重新授权。"}
            正文按需读取。
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
              {current.persistWatch
                ? "重启后继续监听"
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
            {fullFacts ? "保存会话元数据" : "保存元数据供重开读取"}
          </label>
          {current?.canPersistWatch && (
            <label className="flex items-center gap-2 text-label">
              <Checkbox
                checked={persistWatch}
                disabled={busy || !current.sourceAuthorized}
                onCheckedChange={(value) => setWatch(value === true)}
              />
              允许应用重启后继续自动读取并监听此文件
            </label>
          )}
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
                  setWatch(null);
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
            {current?.persistWatch
              ? "停止统一展示会同时撤销跨重启监听，保留本次运行的来源授权与扩展视图。"
              : "停止统一展示会使对应统一查询失效，保留原来源授权与扩展视图。"}
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
