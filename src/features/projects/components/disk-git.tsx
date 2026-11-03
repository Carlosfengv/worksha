import {
  DetailList,
  DetailListItem,
  DetailListLabel,
  DetailListValue,
} from "@/components/ui/detail-list";
import type { ProjectDetail } from "@/features/projects/types";
import { date } from "@/features/sessions/display";
import { Totals } from "./project-facts";

export function DiskGit({ detail }: { detail: ProjectDetail }) {
  const project = detail.project;
  const git = detail.git;
  return (
    <>
      <h3 className="text-title font-medium">会话磁盘与项目身份</h3>
      <Totals totals={project.totals} listener={project.listenerCount} />
      <DetailList>
        {[
          ["项目目录", project.path],
          ["Git 根目录", project.gitRoot ?? "非 Git 或当前不可用"],
          ["分支", git.branch ?? "未知 / 不适用"],
          ["HEAD", git.head ?? "未知 / 尚无提交"],
          [
            "远程仓库",
            git.remote ??
              (git.state === "ready" ? "未配置 origin" : "未知 / 不适用"),
          ],
          ["Worktree 元数据", git.gitDir ?? "未知 / 不适用"],
          ["仓库公共目录", git.commonDir ?? "未知 / 不适用"],
          ["仓库身份", git.repositoryId ?? "未知 / 不适用"],
        ].map(([label, value]) => (
          <DetailListItem key={label}>
            <DetailListLabel>{label}</DetailListLabel>
            <DetailListValue className="break-all">{value}</DetailListValue>
          </DetailListItem>
        ))}
      </DetailList>
      <p className="text-label text-fg-muted">
        不扫描项目源码容量，不执行 Git
        提交、回退或清理。历史写入内容通过原生会话按需查看。
      </p>
      <section className="space-y-3">
        <h3 className="text-title font-medium">最近 Git 提交</h3>
        <p className="text-label text-fg-muted">
          最多 30 条当前 HEAD 历史，不按时间接近归给 Agent。
        </p>
        {git.commits.length ? (
          git.commits.map((commit) => (
            <article
              key={commit.id}
              className="space-y-1 border-b border-border py-3"
            >
              <p className="break-words text-body">{commit.subject}</p>
              <p className="break-all text-label text-fg-subtle">
                {commit.id.slice(0, 12)} · {date(commit.occurredAt)}
              </p>
            </article>
          ))
        ) : (
          <p className="text-body text-fg-muted">
            {git.state === "not_git"
              ? "非 Git 项目没有提交历史。"
              : "没有可用的提交记录。"}
          </p>
        )}
      </section>
    </>
  );
}
