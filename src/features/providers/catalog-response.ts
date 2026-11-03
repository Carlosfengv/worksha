import type {
  ModelDocumentation,
  ModelObservation,
  ProviderModelCatalog,
  ProviderModelInfo,
} from "./types";
import {
  type Decoder,
  array,
  boolean,
  count,
  date,
  nonempty,
  nullable,
  oneOf,
  optional,
  response,
  shape,
  text,
  url,
} from "./response-values";

const documentation: Decoder<ModelDocumentation> = shape({
  fields: array(
    oneOf("tools", "structuredOutput", "jsonOutput", "contextWindow"),
    4,
  ),
  label: nonempty(),
  url,
  checkedAt: date,
  expiresAt: date,
});
const observation: Decoder<ModelObservation> = shape({
  model: nonempty(800),
  client: nonempty(),
  clientVersion: text(),
  checkedAt: date,
  checks: array(
    shape({
      field: oneOf(
        "tools",
        "jsonOutput",
        "jsonReliability",
        "contextMarker",
        "contextCapacity",
        "maxOutputTokens",
        "structuredOutput",
        "vision",
        "reasoning",
      ),
      status: oneOf("passed", "failed", "unknown"),
      detail: text(),
    }),
    64,
  ),
});
const model: Decoder<ProviderModelInfo> = shape({
  id: nonempty(800),
  name: nonempty(800),
  contextWindow: optional(nullable(count)),
  maxInputTokens: optional(nullable(count)),
  maxOutputTokens: optional(nullable(count)),
  capabilities: optional(
    shape({
      tools: nullable(boolean),
      vision: nullable(boolean),
      reasoning: nullable(boolean),
      structuredOutput: nullable(boolean),
      jsonOutput: optional(nullable(boolean)),
    }),
  ),
  documentation: optional(array(documentation, 32)),
  contextAlias: optional(
    shape({
      contextWindow: count,
      upstreamContextWindow: count,
      documentation: array(documentation, 32),
    }),
  ),
});
const catalog: Decoder<ProviderModelCatalog> = shape({
  models: array(model, 2000),
  fetchedAt: date,
  sourceUrl: url,
  truncated: boolean,
  observations: optional(
    shape({
      state: oneOf("ready", "unavailable"),
      models: array(observation, 2000),
    }),
  ),
});
export function providerModelCatalog(value: unknown): ProviderModelCatalog {
  return response(
    (input) => {
      const result = catalog(input);
      if (
        new Set(result.models.map((item) => item.id)).size !==
        result.models.length
      )
        throw Error("duplicate model");
      return result;
    },
    value,
    "模型目录",
  );
}
