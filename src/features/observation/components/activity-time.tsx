import { useEffect, useState } from "react";
import { relativeTime } from "../display";
import { formatDateTime } from "@/shared/lib/date-time";

/** Age advances independently of network reads, including during an error. */
export function ActivityTime({
  value,
  kind,
  title,
}: {
  value: string | null | undefined;
  kind: "request" | "activity" | "sample";
  title?: string;
}) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const update = () => {
      if (document.visibilityState === "visible") setNow(Date.now());
    };
    const timer = window.setInterval(update, 1000);
    document.addEventListener("visibilitychange", update);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", update);
    };
  }, []);
  const valid = value != null && Number.isFinite(Date.parse(value));
  return (
    <time
      dateTime={valid ? value : undefined}
      title={[title, valid ? formatDateTime(value, "full") : "时间未记录"]
        .filter(Boolean)
        .join(" · ")}
    >
      {valid
        ? `${relativeTime(value, now)}${kind === "request" ? "请求" : kind === "sample" ? "测得" : "活动"}`
        : kind === "request"
          ? "请求时间未记录"
          : kind === "sample"
            ? "采样时间未记录"
            : "活动时间未记录"}
    </time>
  );
}
