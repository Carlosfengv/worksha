import { Button } from "@/components/ui/button";
import { projectsApi } from "@/features/projects/api";
import { useState } from "react";

export function ProjectDirectoryAccess({
  projectId,
  onSelected,
}: {
  projectId: string;
  onSelected: () => void;
}) {
  const [loading, setLoading] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  return (
    <div className="space-y-2">
      <p className="text-label text-fg-muted">
        macOS
        可能需要你通过系统选择器允许读取当前项目目录。选择后重新采集，不修改项目文件。
      </p>
      <Button
        variant="tertiary"
        loading={loading}
        onClick={async () => {
          setLoading(true);
          setMessage(null);
          try {
            const selected =
              await projectsApi.selectProjectDirectory(projectId);
            setMessage(
              selected
                ? "目录已选择，正在重新采集；以新快照为准。"
                : "已取消目录选择。",
            );
            if (selected) onSelected();
          } catch (reason) {
            setMessage(
              reason instanceof Error ? reason.message : String(reason),
            );
          } finally {
            setLoading(false);
          }
        }}
      >
        选择项目目录并重试
      </Button>
      {message && (
        <p role="status" className="text-label text-fg-muted">
          {message}
        </p>
      )}
    </div>
  );
}
