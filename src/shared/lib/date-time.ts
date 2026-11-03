import { parseReportedDate } from "./reported-date";

export const DISPLAY_LOCALE = "zh-CN";
export const DISPLAY_TIME_ZONE = "Asia/Shanghai";
type DateValue = string | number | Date | null | undefined;
const compact = new Intl.DateTimeFormat(DISPLAY_LOCALE, {
  timeZone: DISPLAY_TIME_ZONE,
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});
const full = new Intl.DateTimeFormat(DISPLAY_LOCALE, {
  timeZone: DISPLAY_TIME_ZONE,
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  hour12: false,
});
const calendar = new Intl.DateTimeFormat(DISPLAY_LOCALE, {
  timeZone: DISPLAY_TIME_ZONE,
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
});
function parsed(value: DateValue) {
  if (value == null || value === "") return null;
  if (typeof value === "string") return parseReportedDate(value);
  const time = value instanceof Date ? value : new Date(value);
  return Number.isFinite(time.getTime()) ? time : null;
}
export function formatDateTime(
  value: DateValue,
  detail: "compact" | "full" = "compact",
) {
  // A calendar date has no reported clock time; do not invent one.
  if (typeof value === "string" && /^\d{4}-\d{2}-\d{2}$/.test(value))
    return formatCalendarDate(value);
  const time = parsed(value);
  return time ? (detail === "full" ? full : compact).format(time) : "时间未知";
}
export function formatCalendarDate(value: DateValue) {
  const time = parsed(value);
  return time ? calendar.format(time) : "时间未知";
}
