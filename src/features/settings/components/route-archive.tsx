import { useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { settingsApi } from "../api";

type Preview = Awaited<ReturnType<typeof settingsApi.archiveRouteSessions>>;

export function RouteArchive({ storeId }: { storeId: string }) {
  const [preview, setPreview] = useState<Preview | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");

  async function run(confirmed?: Preview) {
    setBusy(true);
    setMessage("");
    try {
      const result = await settingsApi.archiveRouteSessions(
        confirmed
          ? { storeId, confirmation: confirmed.confirmation }
          : { storeId },
      );
      if (confirmed) {
        setPreview(null);
        setMessage(
          `已归档 ${result.sessions} 个会话的 ${result.leases} 条路由租约。`,
        );
      } else if (result.sessions > 0) {
        setPreview(result);
      } else {
        setMessage("当前没有可归档的路由租约。");
      }
    } catch (error) {
      setPreview(null);
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="space-y-3">
      <h2 className="text-title font-medium">历史路由租约</h2>
      <p className="text-body text-fg-muted">
        归档可腾出路由记录空间。原始会话和请求记录会保留，已归档的工具续调将被拒绝。
      </p>
      <Button variant="secondary" disabled={busy} onClick={() => void run()}>
        预览可归档会话
      </Button>
      {message && (
        <p role="status" className="text-body">
          {message}
        </p>
      )}
      <Dialog
        open={preview !== null}
        onOpenChange={(open) => {
          if (!open && !busy) setPreview(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>归档历史路由租约</DialogTitle>
            <DialogDescription>
              {preview &&
                `将归档 ${preview.sessions} 个会话的 ${preview.leases} 条路由租约。归档后，旧工具续调将被拒绝。继续会话需要可核对的完整历史和新的用户指令。`}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="secondary"
              disabled={busy}
              onClick={() => setPreview(null)}
            >
              取消
            </Button>
            <Button
              disabled={busy || !preview}
              onClick={() => preview && void run(preview)}
            >
              {busy ? "正在归档" : "确认归档"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
