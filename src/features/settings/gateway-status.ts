import { DesktopError } from "@/shared/lib/desktop-command";
import { gatewayUuid, gatewayUuid4 } from "./gateway-operation";
import type { GatewayClient, GatewayStatus } from "./types";
const record = (value: unknown): Record<string, unknown> => {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw Error("shape");
  return value as Record<string, unknown>;
};
const integer = (v: unknown, min: number, max: number): v is number =>
  typeof v === "number" && Number.isInteger(v) && v >= min && v <= max;
export function gatewayStatus(value: unknown): GatewayStatus {
  try {
    const r = record(value);
    if (
      typeof r.persistent !== "boolean" ||
      !(r.listen === null || typeof r.listen === "string") ||
      !Array.isArray(r.clients) ||
      r.clients.length > 128 ||
      !(r.concurrency === null || integer(r.concurrency, 1, 64)) ||
      !(r.active === null || integer(r.active, 0, 64)) ||
      ["key", "apiKey", "token", "credential"].some(
        (key) => r[key] !== undefined,
      )
    )
      throw Error("status");
    const ids = new Set<string>();
    const clients: GatewayClient[] = r.clients.map((value) => {
      const c = record(value);
      if (
        !gatewayUuid(c.id) ||
        ids.has(c.id) ||
        typeof c.name !== "string" ||
        !c.name.trim() ||
        [...c.name].length > 80 ||
        typeof c.agent !== "string" ||
        typeof c.providerId !== "string" ||
        !c.providerId ||
        !Array.isArray(c.models) ||
        !c.models.length ||
        c.models.length > 200 ||
        !c.models.every((m) => typeof m === "string" && m.length > 0) ||
        !integer(c.concurrency, 1, 16) ||
        !integer(c.active, 0, c.concurrency) ||
        typeof c.enabled !== "boolean" ||
        typeof c.createdAt !== "string" ||
        !Number.isFinite(Date.parse(c.createdAt)) ||
        ["key", "apiKey", "token", "credential"].some(
          (key) => c[key] !== undefined,
        )
      )
        throw Error("client");
      ids.add(c.id);
      return {
        id: c.id,
        name: c.name,
        agent: c.agent,
        providerId: c.providerId,
        models: [...c.models] as string[],
        concurrency: c.concurrency,
        active: c.active,
        enabled: c.enabled,
        createdAt: c.createdAt,
      };
    });
    const status: GatewayStatus = {
      listen: r.listen as string | null,
      persistent: r.persistent,
      concurrency: r.concurrency as number | null,
      active: r.active as number | null,
      clients,
    };
    if (r.writeProtocol !== undefined) {
      const p = record(r.writeProtocol);
      if (p.version === 1) {
        if (
          !gatewayUuid4(p.storeId) ||
          p.secretReplay !== false ||
          !integer(p.maxClients, 1, 128)
        )
          throw Error("protocol");
        status.writeProtocol = {
          version: 1,
          storeId: p.storeId,
          maxClients: p.maxClients,
          secretReplay: false,
        };
      }
    }
    return status;
  } catch {
    throw new DesktopError({
      code: "transport_invalid_response",
      outcomeUnknown: false,
      message: "网关状态与来源不可核对，客户端修改已停用；请重新读取原后台。",
    });
  }
}
