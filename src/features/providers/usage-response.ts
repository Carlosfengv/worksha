import type {
  ProviderCost,
  ProviderUsage,
  ProviderUsageQuery,
  ProviderUsageTotals,
} from "./types";
import {
  type Decoder,
  array,
  count,
  date,
  dictionary,
  nonempty,
  nullable,
  number,
  omitted,
  oneOf,
  optional,
  response,
  shape,
  text,
} from "./response-values";

const usage = shape({
  inputTokens: nullable(count),
  outputTokens: nullable(count),
  cachedInputTokens: nullable(count),
  cacheWriteInputTokens: nullable(count),
  reasoningOutputTokens: nullable(count),
  totalTokens: nullable(count),
});
const amount = shape({ currency: nonempty(32), amount: number, calls: count });
const costTotals = shape({
  actual: array(amount),
  estimated: array(amount),
  unknownCalls: count,
});
const cost: Decoder<ProviderCost> = shape({
  kind: oneOf("actual", "estimated", "unknown"),
  amount: nullable(number),
  currency: nullable(text(32)),
  reason: nullable(text()),
  source: nullable(text()),
  price: nullable(
    shape({
      version: nonempty(),
      checkedAt: date,
      validFrom: date,
      validUntil: date,
      model: nonempty(800),
      period: nonempty(),
      inputPerMillion: number,
      cachedInputPerMillion: number,
      outputPerMillion: number,
    }),
  ),
  components: array(
    shape({
      kind: nonempty(),
      tokens: count,
      perMillion: number,
      amount: number,
    }),
  ),
});
const totals: Decoder<ProviderUsageTotals> = shape({
  cost: costTotals,
  unknownCostReasons: optional(dictionary(count)),
  calls: count,
  errors: count,
  inProgress: count,
  unknownOutcomes: count,
  usage,
  reported: shape({
    inputTokens: optional(count),
    outputTokens: optional(count),
    cachedInputTokens: optional(count),
    cacheWriteInputTokens: optional(count),
    reasoningOutputTokens: optional(count),
    totalTokens: optional(count),
  }),
  lastSeen: nullable(date),
  latencyP95Ms: nullable(number),
  latencySamples: count,
  ttftP95Ms: nullable(number),
  ttftSamples: count,
});
const query: Decoder<ProviderUsageQuery> = shape({
  source: oneOf("gateway", "native"),
  agent: omitted(text()),
  days: omitted(count),
  providerId: omitted(text()),
  model: omitted(text(800)),
  cursor: omitted(text()),
});
const snapshot: Decoder<ProviderUsage> = shape({
  query,
  asOf: date,
  sourceAsOf: date,
  sourceState: nonempty(),
  summary: totals,
  providers: array(shape({ providerId: nonempty(), totals })),
  unattributed: totals,
  selected: nullable(totals),
  modelOptions: array(nullable(text(800))),
  models: array(shape({ model: nullable(text(800)), totals })),
  providerModels: optional(
    array(
      shape({ providerId: nonempty(), model: nullable(text(800)), totals }),
    ),
  ),
  calls: array(
    shape({
      id: nonempty(),
      providerId: nullable(text()),
      provider: nullable(text()),
      attribution: nonempty(),
      agentId: nonempty(),
      agentProductId: nullable(text()),
      model: nullable(text(800)),
      time: nullable(date),
      status: nonempty(),
      usage,
      cost,
      durationMs: nullable(number),
      ttftMs: nullable(number),
      sessionId: nullable(text()),
      requestId: nullable(text()),
    }),
  ),
  totalCalls: count,
  nextCursor: nullable(text()),
  missingTime: count,
});
export function providerUsage(
  value: unknown,
  expected: ProviderUsageQuery,
): ProviderUsage {
  return response(
    (input) => {
      const result = snapshot(input),
        request = query(expected);
      for (const field of [
        "source",
        "agent",
        "days",
        "providerId",
        "model",
        "cursor",
      ] as const)
        if (result.query[field] !== request[field])
          throw Error("different usage scope");
      return result;
    },
    value,
    "供应商用量",
  );
}
