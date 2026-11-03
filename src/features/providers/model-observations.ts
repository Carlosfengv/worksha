import type { ModelObservation, ModelObservationField } from "./types";

export const observationFields = [
  ["tools", "工具调用"],
  ["jsonOutput", "JSON 模式"],
  ["jsonReliability", "JSON 稳定性"],
  ["contextMarker", "[1M] 标记"],
  ["contextCapacity", "实际上下文容量"],
  ["maxOutputTokens", "最大输出"],
  ["structuredOutput", "JSON Schema"],
  ["vision", "图片输入"],
  ["reasoning", "推理思考"],
] as const satisfies readonly (readonly [ModelObservationField, string])[];

// Only the spelling of Claude Code's suffix is case insensitive; no base-to-alias inheritance.
export const observationModelKey = (model: string) =>
  model.replace(/\[1m\]$/i, "[1m]");

export function observationStatus(
  observations: ModelObservation[],
  field: ModelObservationField,
) {
  const checks = observations.flatMap((o) =>
    o.checks.filter((c) => c.field === field),
  );
  if (checks.some((c) => c.status === "failed")) return "未通过";
  if (!checks.length || checks.some((c) => c.status === "unknown"))
    return "未知";
  return "通过（样本）";
}
