import type {
  ObservationBinding,
  Overview,
  OverviewQuery,
  RequestPage,
} from "@/features/observation/types";
import { command } from "@/shared/lib/desktop-command";
import type {
  RequestDashboard,
  RequestDetailResponse,
  RequestQuery,
} from "./request-types";

type LoopAmounts = { tokens: number | null; usdMicros: number | null };
export interface LoopAlert {
  bindingId: string;
  sessionId: string;
  agentId: string;
  bindingName: string | null;
  turnDigest: string;
  overrideEpoch: number;
  status: "warned" | "tripped";
  reason:
    "toolErrorStreak" | "actionPingPong" | "unattendedBurn" | "dedupCapacity";
  continuationSteps: number;
  consecutiveToolErrors: number;
  repeatedActions: number;
  usage: {
    committed: LoopAmounts;
    reserved: LoopAmounts;
    frozen: LoopAmounts;
    usdUnknown: number;
  };
}

export interface LoopOverride {
  storeId: string;
  operationId: string;
  bindingId: string;
  sessionId: string;
  expectedTurnDigest: string;
  expectedOverrideEpoch: number;
}

export const observationApi = {
  loopGuards(): Promise<{ storeId: string; alerts: LoopAlert[] }> {
    return command("query_loop_guards");
  },
  overrideLoopGuard(
    input: LoopOverride,
  ): Promise<{ operationId: string; overrideEpoch: number }> {
    return command("override_loop_guard", { input });
  },
  requestDashboard(query: RequestQuery): Promise<RequestDashboard> {
    return command("query_request_dashboard", { query });
  },
  requestDetail(id: string): Promise<RequestDetailResponse> {
    return command("get_request_detail", { id });
  },
  exportRequests(snapshot: string): Promise<{
    status: "saved" | "cancelled";
    count: number;
    fileName?: string;
  }> {
    return command("export_request_csv", { snapshot });
  },
  overview(query: OverviewQuery): Promise<Overview> {
    return command("query_overview", { query });
  },
  requests(agent?: string, offset?: number, id?: string): Promise<RequestPage> {
    return command("query_requests", {
      agent: agent ?? null,
      offset: offset ?? null,
      id: id ?? null,
    });
  },
  observationBinding(
    agentId: string,
    providerId: string,
    expectedRevision: number,
    enabled: boolean,
  ): Promise<ObservationBinding[]> {
    return command("save_observation_binding", {
      agentId,
      providerId,
      expectedRevision,
      enabled,
    });
  },
};
