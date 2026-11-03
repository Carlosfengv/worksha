type Schema =
  | boolean
  | {
      $ref?: string;
      $defs?: Record<string, Schema>;
      type?: string | string[];
      const?: unknown;
      enum?: unknown[];
      anyOf?: Schema[];
      oneOf?: Schema[];
      allOf?: Schema[];
      properties?: Record<string, Schema>;
      required?: string[];
      additionalProperties?: Schema;
      items?: Schema;
      minimum?: number;
      maximum?: number;
    };
export function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function accepts(
  schema: Schema,
  value: unknown,
  root: Schema,
  depth = 0,
): boolean {
  if (depth > 40) return false;
  if (typeof schema === "boolean") return schema;
  if (schema.$ref) {
    if (!schema.$ref.startsWith("#/$defs/") || typeof root === "boolean")
      return false;
    const target = root.$defs?.[schema.$ref.slice(8)];
    return target !== undefined && accepts(target, value, root, depth + 1);
  }
  if (schema.const !== undefined && value !== schema.const) return false;
  if (schema.enum && !schema.enum.includes(value)) return false;
  if (
    schema.anyOf &&
    !schema.anyOf.some((s) => accepts(s, value, root, depth + 1))
  )
    return false;
  if (
    schema.oneOf &&
    schema.oneOf.filter((s) => accepts(s, value, root, depth + 1)).length !== 1
  )
    return false;
  if (
    schema.allOf &&
    !schema.allOf.every((s) => accepts(s, value, root, depth + 1))
  )
    return false;
  if (Array.isArray(schema.type))
    return schema.type.some((type) =>
      accepts({ ...schema, type }, value, root, depth + 1),
    );
  if (schema.type === "null") return value === null;
  if (schema.type === "string") return typeof value === "string";
  if (schema.type === "boolean") return typeof value === "boolean";
  if (schema.type === "integer" || schema.type === "number")
    return (
      typeof value === "number" &&
      Number.isFinite(value) &&
      (schema.type !== "integer" || Number.isSafeInteger(value)) &&
      (schema.minimum === undefined || value >= schema.minimum) &&
      (schema.maximum === undefined || value <= schema.maximum)
    );
  if (schema.type === "array")
    return (
      Array.isArray(value) &&
      value.length <= 10240 &&
      value.every((v) => accepts(schema.items ?? true, v, root, depth + 1))
    );
  if (schema.type === "object") {
    if (!object(value) || schema.required?.some((name) => !(name in value)))
      return false;
    return Object.entries(value).every(([name, value]) => {
      const target = Object.hasOwn(schema.properties ?? {}, name)
        ? schema.properties![name]
        : (schema.additionalProperties ?? true);
      return accepts(target, value, root, depth + 1);
    });
  }
  return true;
}
export function parseSchema<T>(schema: unknown, value: unknown): T {
  if (!accepts(schema as Schema, value, schema as Schema))
    throw new Error("高级扩展响应不符合已登记接口。");
  return value as T;
}

export function acceptsSchema(schema: unknown, value: unknown) {
  return accepts(schema as Schema, value, schema as Schema);
}
