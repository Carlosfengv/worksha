import { localConnectionsApi } from "@/features/agents/api";
import { observationApi, type LoopAlert } from "@/features/observation/api";

export function installLoopGuards() {
  Object.defineProperty(globalThis, "isTauri", { value: true });
  const usage: LoopAlert["usage"] = {
    committed: { tokens: null, usdMicros: null },
    reserved: { tokens: null, usdMicros: null },
    frozen: { tokens: null, usdMicros: null },
    usdUnknown: 0,
  };
  const base = {
    agentId: "claude",
    turnDigest: "fixture-turn",
    overrideEpoch: 2,
    continuationSteps: 5,
    consecutiveToolErrors: 5,
    repeatedActions: 1,
    usage,
  };
  let alerts: LoopAlert[] = [
    {
      ...base,
      bindingId: "ordinary",
      bindingName: "Main binding",
      sessionId: "claude:loop",
      status: "tripped",
      reason: "toolErrorStreak",
    },
    {
      ...base,
      bindingId: "full",
      bindingName: "Full binding",
      sessionId: "chat:full",
      status: "tripped",
      reason: "dedupCapacity",
    },
    {
      ...base,
      bindingId: "warning",
      bindingName: "Warning binding",
      sessionId: "chat:warning",
      status: "warned",
      reason: "actionPingPong",
      repeatedActions: 4,
    },
  ];
  localConnectionsApi.warnings = async () => ({
    storeId: "fixture-store",
    warnings: [],
  });
  observationApi.loopGuards = async () => ({
    storeId: "fixture-store",
    alerts,
  });
  observationApi.overrideLoopGuard = async (input) => {
    if (
      input.storeId !== "fixture-store" ||
      !input.operationId ||
      input.bindingId !== "ordinary" ||
      input.sessionId !== "claude:loop" ||
      input.expectedTurnDigest !== "fixture-turn" ||
      input.expectedOverrideEpoch !== 2
    ) {
      throw Error("Wrong override scope");
    }
    alerts = alerts.filter((alert) => alert.bindingId !== input.bindingId);
    return { operationId: input.operationId, overrideEpoch: 3 };
  };
}
