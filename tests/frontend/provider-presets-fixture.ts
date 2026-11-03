import type { PresetAction } from "../../contracts/domain/provider-presets/types";
import {
  builtinPresetAdvice,
  presetKey,
  type PresetAdvice,
} from "@/features/providers/preset-advice";
import { invalidateCapabilityAccess } from "@/shared/lib/capability-access";
export function createProviderPresetFixture() {
  const control = {
    calls: [] as PresetAction[],
    count: 1,
    disabled: false,
    failList: false,
    failResolve: false,
    hold: null as "list" | "resolve" | null,
    bad: null as "identity" | "protocol" | "extra" | "duplicate" | null,
    release: null as (() => void) | null,
    invalidate: invalidateCapabilityAccess,
  };
  const items = (): PresetAdvice[] =>
    Array.from({ length: control.disabled ? 0 : control.count }, (_, i) => ({
      origin: {
        kind: "external",
        pluginId:
          i < 16
            ? "org.example.preset-fixture"
            : `org.example.preset-fixture-${Math.floor(i / 16)}`,
        digest: "a".repeat(64),
        presetId: `preset-${(i % 16) + 1}`,
      },
      name: i === 0 ? "OpenAI" : `外部供应商 ${String(i + 1).padStart(4, "0")}`,
      sourceName: "Preset Fixture",
      protocol: "responses",
      endpoints: { responses: `https://preset-${i + 1}.invalid/v1` },
      model: "preset-starter",
      documents: null,
    }));
  const handle = async (action: PresetAction): Promise<unknown> => {
    control.calls.push(structuredClone(action));
    if (
      (action.kind === "list" && control.failList) ||
      (action.kind === "resolve" && (control.failResolve || control.disabled))
    )
      throw {
        code: "plugin_disabled",
        message: "合成预设来源暂不可用",
        outcomeUnknown: false,
      };
    const advice =
      action.kind === "resolve"
        ? [...builtinPresetAdvice, ...items()].find(
            (p) => presetKey(p.origin) === presetKey(action.origin),
          )
        : null;
    if (action.kind === "resolve" && !advice) throw Error("合成预设身份不存在");
    const reply =
      action.kind === "list"
        ? {
            kind: "listed",
            storeId: action.storeId,
            items: structuredClone([...builtinPresetAdvice, ...items()]),
            issues: [],
          }
        : {
            kind: "resolved",
            storeId: action.storeId,
            advice: structuredClone(advice!),
          };
    if (control.hold === action.kind)
      await new Promise<void>((resolve) => {
        control.release = resolve;
      });
    if (reply.kind === "resolved" && "advice" in reply && reply.advice) {
      if (control.bad === "identity")
        reply.advice.origin = {
          kind: "external",
          pluginId: "org.example.other",
          digest: "b".repeat(64),
          presetId: "other",
        };
      if (control.bad === "protocol")
        (reply.advice as { protocol: string }).protocol = "future";
      if (control.bad === "extra")
        Object.assign(reply.advice, { credentialRef: "forbidden" });
    }
    if (control.bad === "duplicate" && "items" in reply && reply.items)
      reply.items.push(reply.items[0]);
    return reply;
  };
  return { control, handle };
}
declare global {
  interface Window {
    providerPresetsFixture: ReturnType<
      typeof createProviderPresetFixture
    >["control"];
  }
}
