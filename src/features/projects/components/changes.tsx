import { useAgentDirectory } from "@/shared/hooks/use-agent-directory";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Button } from "@/components/ui/button";
import { projectDiffResource } from "../queries";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import type {
  FileWrite,
  GitFile,
  ProjectDetail,
} from "@/features/projects/types";
import { date, number, sessionLink } from "@/features/sessions/display";
import { useEffect, useState } from "react";
import { projectLink } from "../links";
import { recordAgentKey } from "@/shared/types/agent";

export function Writes({
  items,
  anchor,
  projectId,
  agent,
  scope,
}: {
  items: FileWrite[];
  anchor?: string;
  projectId: string;
  agent?: string;
  scope?: string;
}) {
  const { agentNames } = useAgentDirectory();
  const [limit, setLimit] = useState(() =>
    Math.max(40, items.findIndex((item) => item.id === anchor) + 1),
  );
  useEffect(() => {
    if (anchor)
      document
        .getElementById(`write-${anchor}`)
        ?.scrollIntoView({ block: "nearest" });
  }, [anchor]);
  return (
    <section className="space-y-4">
      <h3 className="text-title font-medium">Agent 成功写入历史</h3>
      {anchor && (
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-label text-fg-muted">
            {items.some((item) => item.id === anchor)
              ? "已定位写入；同时保留项目其他记录"
              : "该写入不在所选日期或当前来源中"}
          </span>
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => {
              window.location.hash = projectLink(
                projectId,
                "changes",
                undefined,
                agent,
                scope,
              );
            }}
          >
            取消定位
          </Button>
        </div>
      )}
      <p className="text-label text-fg-muted">
        仅识别明确的 Write / Edit / apply_patch 或原生 FileChange
        成功事件；命令包装与模型文字不能作为证据。历史写入与当前 Git
        净差异分开。
      </p>
      {!items.length ? (
        <p className="text-body text-fg-muted">
          未采集到可确认的成功写入，不能据此推断没有修改。
        </p>
      ) : (
        <Accordion
          className="w-full"
          type="multiple"
          defaultValue={anchor ? [anchor] : []}
        >
          <div className="space-y-2">
            {items.slice(0, limit).map((item) => (
              <AccordionItem
                key={item.id}
                value={item.id}
                id={`write-${item.id}`}
              >
                <AccordionTrigger>
                  {agentNames[recordAgentKey(item)]} · {item.tool} ·{" "}
                  {item.files.length} 个文件 · {date(item.occurredAt)}
                  {anchor === item.id ? " · 已定位" : ""}
                </AccordionTrigger>
                <AccordionContent>
                  <div className="space-y-3 p-3">
                    <p className="text-label text-fg-muted">
                      {item.evidence} · 来源序号 {item.sourceLine}
                    </p>
                    {item.files.map((file, index) => (
                      <div
                        key={`${file.path}:${index}`}
                        className="break-all text-body"
                      >
                        {file.operation} · {file.path}
                        <p className="text-label text-fg-subtle">
                          当时新增 {number(file.added)} / 删除{" "}
                          {number(file.removed)} 行
                        </p>
                      </div>
                    ))}
                    <Button
                      variant="tertiary"
                      size="sm"
                      onClick={() => {
                        window.location.hash = sessionLink(item.sessionId, {
                          agent: recordAgentKey(item),
                          anchor: item.id,
                        });
                      }}
                    >
                      定位源调用与当时差异
                    </Button>
                  </div>
                </AccordionContent>
              </AccordionItem>
            ))}
          </div>
        </Accordion>
      )}
      {items.length > limit && (
        <Button variant="tertiary" onClick={() => setLimit((v) => v + 40)}>
          加载更多写入
        </Button>
      )}
    </section>
  );
}

export function GitChanges({
  projectId,
  detail,
}: {
  projectId: string;
  detail: ProjectDetail;
}) {
  return (
    <section className="space-y-4">
      <h3 className="text-title font-medium">Git 当前工作区</h3>
      <p className="text-label text-fg-muted">
        采集于 {date(detail.git.asOf)} · 相对 HEAD
        的当前状态，不受历史日期筛选影响。当前变更的 Agent 归属待确认。
      </p>
      {detail.git.error && (
        <p role="status" className="text-body text-fg-muted">
          {detail.git.error}
        </p>
      )}
      {detail.git.state === "not_git" ? (
        <p className="text-body text-fg-muted">此目录不是 Git 项目。</p>
      ) : detail.git.state === "unavailable" ? (
        <p className="text-body text-fg-muted">
          Git 当前不可读取，变更数量未知。
        </p>
      ) : detail.git.files.length ? (
        <Accordion className="w-full" type="multiple">
          <div className="space-y-2">
            {detail.git.files.map((file) => (
              <AccordionItem key={file.id} value={file.id}>
                <AccordionTrigger>
                  <span className="break-all">
                    {file.indexStatus}
                    {file.worktreeStatus} · {file.path} · 归属待确认
                  </span>
                </AccordionTrigger>
                <AccordionContent>
                  <FileDiff projectId={projectId} file={file} />
                </AccordionContent>
              </AccordionItem>
            ))}
          </div>
        </Accordion>
      ) : (
        <p className="text-body text-fg-muted">当前快照未发现工作区变更。</p>
      )}
    </section>
  );
}

export function FileDiff({
  projectId,
  file,
}: {
  projectId: string;
  file: GitFile;
}) {
  const [opened, setOpened] = useState(false);
  const resource = useSharedQuery(
    projectDiffResource,
    { projectId, fileId: file.id },
    opened,
  );
  const { data: diff, error, loading } = resource;
  return (
    <div className="space-y-3 p-3">
      <p className="text-label text-fg-muted">
        暂存区 {file.indexStatus.trim() || "无变更"} · 工作区{" "}
        {file.worktreeStatus.trim() || "无变更"}
        {file.previousPath ? ` · 原路径 ${file.previousPath}` : ""}
        。查看内容按需读取，不持久化。
      </p>
      <Button
        variant="tertiary"
        size="sm"
        loading={loading}
        onClick={() => {
          if (opened) void resource.reload();
          else setOpened(true);
        }}
      >
        {diff ? "重新读取当前差异" : "查看当前差异"}
      </Button>
      {error && (
        <p role="alert" className="text-body text-fg-muted">
          {error}
        </p>
      )}
      {diff && (
        <>
          <p className="text-label text-fg-muted">
            {date(diff.asOf)} ·{" "}
            {diff.state === "partial"
              ? "内容已截断"
              : (diff.reason ?? "相对 HEAD 的当前净差异")}
          </p>
          {diff.text && (
            <pre className="max-h-96 overflow-auto whitespace-pre-wrap break-all rounded-lg border border-border p-3 font-mono text-label">
              {diff.text}
            </pre>
          )}
        </>
      )}
    </div>
  );
}
