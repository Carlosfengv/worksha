import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { domainApi } from "./api";

/** Mounted for one frozen token; pagination keeps the same export selection. */
export function RequestExport({ snapshotToken }: { snapshotToken: string }) {
  const running = useRef(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const returnFocus = useRef(false);
  const [state, setState] = useState({
    busy: false,
    error: false,
    message: "",
  });
  useEffect(() => {
    if (state.busy || !returnFocus.current) return;
    returnFocus.current = false;
    const button = trigger.current;
    const active = document.activeElement;
    // The native panel can return focus to the WebView while the trigger is
    // disabled. Restore it after this commit; respect any later user focus.
    if (
      button &&
      document.hasFocus() &&
      (active === document.body ||
        active === document.documentElement ||
        active === button)
    )
      button.focus();
  }, [state.busy, state.message]);
  const save = async () => {
    if (running.current) return;
    returnFocus.current = document.activeElement === trigger.current;
    running.current = true;
    setState({ busy: true, error: false, message: "请选择保存位置。" });
    try {
      const reply = await domainApi.exportRequests(snapshotToken);
      setState({
        busy: false,
        error: false,
        message:
          reply.status === "cancelled"
            ? "已取消保存。"
            : `已保存冻结范围内 ${BigInt(reply.info.count).toLocaleString("zh-CN")} 条${reply.info.appliedQuery.unit === "model-response" ? "模型响应" : "网关逻辑请求"}${reply.fileName ? `至 ${reply.fileName}` : ""}，来源状态和未知部分见文件中的范围记录。`,
      });
    } catch (error) {
      setState({
        busy: false,
        error: true,
        message:
          error instanceof Error
            ? error.message
            : "导出结果暂不可读取，请核查目标文件。",
      });
    } finally {
      running.current = false;
    }
  };
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-2 text-label">
      <Button
        ref={trigger}
        size="sm"
        variant="tertiary"
        loading={state.busy}
        onClick={() => void save()}
      >
        导出冻结范围 CSV
      </Button>
      <p
        className="min-w-0 flex-1 basis-48 break-words"
        role={state.error ? "alert" : "status"}
      >
        {state.message || "导出完整匹配范围，保留当前筛选、排序和统计单位。"}
      </p>
    </div>
  );
}
