import { Button } from "@/components/ui/button";
import { number } from "../display";

export function SessionPagination({
  loading,
  previous,
  next,
  first,
  count,
  total,
  partial = false,
  onPrevious,
  onNext,
  onFirst,
}: {
  loading: boolean;
  previous: boolean;
  next: boolean;
  first: boolean;
  count: number;
  total?: number;
  partial?: boolean;
  onPrevious: () => void;
  onNext: () => void;
  onFirst: () => void;
}) {
  return (
    <nav
      aria-label="会话分页"
      aria-busy={loading}
      className="flex shrink-0 flex-wrap items-center gap-2 border-t border-border p-3"
    >
      <Button
        variant="tertiary"
        size="sm"
        disabled={loading || !previous}
        onClick={onPrevious}
      >
        上一页会话
      </Button>
      <Button
        variant="tertiary"
        size="sm"
        disabled={loading || !next}
        onClick={onNext}
      >
        下一页会话
      </Button>
      <Button
        variant="ghost"
        size="sm"
        disabled={loading || first}
        onClick={onFirst}
      >
        回到第一页
      </Button>
      <span role="status" className="text-label text-fg-subtle">
        {total === undefined ? (
          "会话数量尚未核查"
        ) : (
          <>
            本页 {count} 个 · {partial ? "已读取" : "共"} {number(total)} 个
          </>
        )}
        {loading ? " · 正在读取…" : ""}
      </span>
      {partial && (
        <p role="status" className="text-label text-fg-muted">
          部分来源尚未完成核查，数量仅含已读取记录。
        </p>
      )}
    </nav>
  );
}
