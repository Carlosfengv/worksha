// Generate TypeScript from the same schemars output used by the Rust contract test.
import fs from "node:fs";
import assert from "node:assert/strict";
const contributions = process.argv.includes("--contributions");
const objectSummary = process.argv.includes("--object-summary");
const requestExport = process.argv.includes("--request-export");
const objectContributions = process.argv.includes("--object-contributions");
const domain = process.argv.includes("--domain");
const facts = process.argv.includes("--facts");
const provider = process.argv.includes("--provider");
const config = process.argv.includes("--config");
const protocol = process.argv.includes("--protocol");
const providerAccess = process.argv.includes("--provider-access");
const providerConnection = process.argv.includes("--provider-connection");
const providerSourceOperation = process.argv.includes(
  "--provider-source-operation",
);
const presets = process.argv.includes("--provider-presets");
const pluginPolicy = process.argv.includes("--policy");
const routingPolicy = process.argv.includes("--routing-policy");
const policyRecovery = process.argv.includes("--policy-recovery");
const draftRecovery = process.argv.includes("--provider-draft-recovery");
const draftGrant = process.argv.includes("--provider-draft-grant");
const draftCatalog = process.argv.includes("--provider-draft-catalog");
const providerSource = process.argv.includes("--provider-source");
const sourcePlugins = process.argv.includes("--sources");
const root = policyRecovery
  ? "contracts/domain/routing-policy-recovery"
  : requestExport
    ? "contracts/domain/request-export"
    : objectSummary
      ? "contracts/domain/object-summary"
      : objectContributions
        ? "contracts/plugins/objects"
        : pluginPolicy
          ? "contracts/plugins/routing"
          : routingPolicy
            ? "contracts/domain/routing-policy"
            : presets
              ? "contracts/domain/provider-presets"
              : draftRecovery
                ? "contracts/domain/provider-draft-recovery"
                : draftGrant
                  ? "contracts/domain/provider-draft-grant"
                  : draftCatalog
                    ? "contracts/domain/provider-draft-catalog"
                    : providerSourceOperation
                      ? "contracts/domain/provider-source-operation"
                      : providerSource
                        ? "contracts/domain/provider-source"
                        : providerConnection
                          ? "contracts/domain/provider-connection"
                          : providerAccess
                            ? "contracts/domain/provider-access"
                            : protocol
                              ? "contracts/domain/protocol"
                              : config
                                ? "contracts/domain/config"
                                : provider
                                  ? "contracts/domain/provider"
                                  : sourcePlugins
                                    ? "contracts/plugins/sources"
                                    : facts
                                      ? "contracts/domain/facts"
                                      : domain
                                        ? "contracts/domain"
                                        : contributions
                                          ? "contracts/plugins/contributions"
                                          : process.argv.includes("--extensions")
                                            ? "contracts/plugins/extensions"
                                            : "contracts/plugins";
const schemas = fs
  .readdirSync(root)
  .filter((n) => n.endsWith(".schema.json"))
  .sort()
  .map((n) => JSON.parse(fs.readFileSync(`${root}/${n}`, "utf8")));
assert(
  schemas.length >=
    (policyRecovery
      ? 4
      : requestExport
        ? 6
        : objectSummary
          ? 2
          : objectContributions
            ? 4
            : pluginPolicy
              ? 9
              : routingPolicy || presets
                ? 4
                : draftRecovery
                  ? 4
                  : draftGrant
                    ? 4
                    : draftCatalog
                      ? 4
                      : providerSourceOperation
                        ? 3
                        : providerSource
                          ? 4
                          : providerConnection
                            ? 3
                            : providerAccess
                              ? 8
                              : protocol
                                ? 4
                                : config || provider
                                  ? 5
                                  : sourcePlugins
                                    ? 8
                                    : facts
                                      ? 3
                                      : domain
                                        ? 8
                                        : contributions
                                          ? 2
                                          : process.argv.includes("--extensions")
                                            ? 8
                                            : 19),
  "Generate schemas with the SDK tool first",
);
const declarations = new Map();
for (const schema of schemas) {
  for (const [name, definition] of Object.entries(schema.$defs ?? {})) {
    const previous = declarations.get(name);
    assert(
      !previous || JSON.stringify(previous) === JSON.stringify(definition),
      `Conflicting definition ${name}`,
    );
    declarations.set(name, definition);
  }
}
for (const schema of schemas) {
  const { $schema, title, $defs, ...definition } = schema;
  void $schema;
  void $defs;
  if (!declarations.has(title)) declarations.set(title, definition);
}
function type(s) {
  if (s === true) return "unknown";
  if (s === false) return "never";
  if (s.$ref) {
    assert(s.$ref.startsWith("#/$defs/"));
    return s.$ref.slice(8);
  }
  if (s.const !== undefined) return JSON.stringify(s.const);
  if (s.enum) return s.enum.map((v) => JSON.stringify(v)).join(" | ");
  if (s.oneOf || s.anyOf)
    return `(${(s.oneOf ?? s.anyOf).map(type).join(" | ")})`;
  if (s.allOf) return `(${s.allOf.map(type).join(" & ")})`;
  if (Array.isArray(s.type))
    return `(${s.type.map((t) => type({ ...s, type: t })).join(" | ")})`;
  if (s.type === "null") return "null";
  if (s.type === "string") return "string";
  if (s.type === "boolean") return "boolean";
  if (s.type === "integer" || s.type === "number") return "number";
  if (s.type === "array") return `Array<${type(s.items ?? true)}>`;
  if (s.type === "object" || s.properties) {
    const fields = Object.entries(s.properties ?? {}).map(
      ([key, value]) =>
        `${JSON.stringify(key)}${s.required?.includes(key) ? "" : "?"}: ${type(value)};`,
    );
    if (s.additionalProperties && typeof s.additionalProperties === "object")
      fields.push(`[key: string]: ${type(s.additionalProperties)};`);
    return `{ ${fields.join(" ")} }`;
  }
  throw new Error(`Unsupported schema: ${JSON.stringify(s)}`);
}
const text = `// Generated from ${root}/*.schema.json. Do not edit.\n${[
  ...declarations,
]
  .sort(([a], [b]) => a.localeCompare(b))
  .map(([name, schema]) => `export type ${name} = ${type(schema)};`)
  .join("\n")}\n`;
const target = `${root}/types.ts`;
if (process.argv.includes("--write")) fs.writeFileSync(target, text);
else
  assert.equal(fs.readFileSync(target, "utf8"), text, "Plugin bindings drift");
if (
  !objectSummary &&
  !requestExport &&
  !objectContributions &&
  !pluginPolicy &&
  !routingPolicy &&
  !policyRecovery &&
  !presets &&
  !draftRecovery &&
  !draftGrant &&
  !draftCatalog &&
  !providerSourceOperation &&
  !providerSource &&
  !providerConnection &&
  !providerAccess &&
  !protocol &&
  !config &&
  !provider &&
  !contributions &&
  !domain &&
  !facts
)
  assert.equal(
    fs.readFileSync(`${root}/plugin.wit`, "utf8"),
    fs.readFileSync("sdk/plugins/wit/plugin.wit", "utf8"),
    "SDK WIT drift",
  );
console.log(
  pluginPolicy
    ? `${schemas.length} pure policy schemas and TypeScript bindings agree`
    : policyRecovery ||
        requestExport ||
        objectSummary ||
        routingPolicy ||
        presets ||
        draftRecovery ||
        draftGrant ||
        draftCatalog ||
        providerSourceOperation ||
        providerSource ||
        providerConnection ||
        providerAccess ||
        protocol ||
        config ||
        provider ||
        domain ||
        facts
      ? `${schemas.length} domain schemas and TypeScript bindings agree`
      : contributions || objectContributions
        ? `${schemas.length} contribution schemas and TypeScript bindings agree`
        : `${schemas.length} plugin schemas, TypeScript bindings and SDK WIT agree`,
);
