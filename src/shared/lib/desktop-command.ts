import { invoke, isTauri } from "@tauri-apps/api/core";

function requireDesktop() {
  if (!isTauri()) throw new Error("请在 Local Agent 桌面应用中连接本机数据。");
}

export class DesktopError extends Error {
  readonly code: string;
  readonly outcomeUnknown: boolean;
  constructor(reason: unknown) {
    let detail: unknown = reason;
    if (typeof reason === "string") {
      try {
        detail = JSON.parse(reason);
      } catch {
        /* Plain native error. */
      }
    }
    const object =
      detail && typeof detail === "object"
        ? (detail as Record<string, unknown>)
        : null;
    super(
      typeof object?.message === "string" ? object.message : String(reason),
    );
    this.code = typeof object?.code === "string" ? object.code : "unavailable";
    this.outcomeUnknown = object?.outcomeUnknown === true;
  }
}
export async function command<T>(
  name: string,
  args?: Record<string, unknown>,
): Promise<T> {
  requireDesktop();
  try {
    return await invoke<T>(name, args);
  } catch (reason) {
    const error = new DesktopError(reason);
    if (error.code === "login_required")
      window.dispatchEvent(new Event("local-agent:login-required"));
    throw error;
  }
}
