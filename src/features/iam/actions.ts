import { useRef, useState } from "react";
import { DesktopError } from "@/shared/lib/desktop-command";

export function field(data: FormData, name: string) {
  return String(data.get(name) ?? "");
}
export function useIamAction() {
  const inFlight = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async (task: () => Promise<unknown>) => {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError(null);
    try {
      await task();
    } catch (reason) {
      if (!(reason instanceof DesktopError && reason.code === "cancelled"))
        setError(
          reason instanceof Error ? reason.message : "操作失败，请重试。",
        );
    } finally {
      setBusy(false);
      inFlight.current = false;
    }
  };
  return { busy, error, run };
}
