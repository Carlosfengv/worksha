import { DesktopError } from "@/shared/lib/desktop-command";
import { parseReportedDate } from "@/shared/lib/reported-date";

export type Decoder<T> = (value: unknown) => T;
const invalid = (): never => {
  throw Error("invalid provider response");
};
export const record = (value: unknown): Record<string, unknown> =>
  value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : invalid();
export const text =
  (max = 16384): Decoder<string> =>
  (value) =>
    typeof value === "string" && value.length <= max && !value.includes("\0")
      ? value
      : invalid();
export const nonempty =
  (max = 2048): Decoder<string> =>
  (value) => {
    const result = text(max)(value);
    return result.trim() ? result : invalid();
  };
export const count: Decoder<number> = (value) =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 0
    ? value
    : invalid();
export const number: Decoder<number> = (value) =>
  typeof value === "number" && Number.isFinite(value) && value >= 0
    ? value
    : invalid();
export const boolean: Decoder<boolean> = (value) =>
  typeof value === "boolean" ? value : invalid();
export const date: Decoder<string> = (value) => {
  const result = nonempty(64)(value);
  return parseReportedDate(result) ? result : invalid();
};
export const url: Decoder<string> = (value) => {
  const result = nonempty(2048)(value),
    parsed = new URL(result);
  return ["http:", "https:"].includes(parsed.protocol) &&
    !parsed.username &&
    !parsed.password
    ? result
    : invalid();
};
export const optional =
  <T>(decode: Decoder<T>): Decoder<T | undefined> =>
  (value) =>
    value === undefined ? undefined : decode(value);
export const nullable =
  <T>(decode: Decoder<T>): Decoder<T | null> =>
  (value) =>
    value === null ? null : decode(value);
// Rust's optional query fields serialize as null; omission has the same meaning.
export const omitted =
  <T>(decode: Decoder<T>): Decoder<T | undefined> =>
  (value) =>
    value == null ? undefined : decode(value);
export const oneOf =
  <const T extends readonly string[]>(...values: T): Decoder<T[number]> =>
  (value) =>
    typeof value === "string" && values.includes(value) ? value : invalid();
export const array =
  <T>(decode: Decoder<T>, max = 100000): Decoder<T[]> =>
  (value) =>
    Array.isArray(value) && value.length <= max ? value.map(decode) : invalid();
export const dictionary =
  <T>(decode: Decoder<T>): Decoder<Record<string, T>> =>
  (value) =>
    Object.fromEntries(
      Object.entries(record(value)).map(([key, item]) => [key, decode(item)]),
    );
export function shape<S extends Record<string, Decoder<unknown>>>(
  schema: S,
): Decoder<{ [K in keyof S]: ReturnType<S[K]> }> {
  return (value) => {
    const input = record(value),
      output: Record<string, unknown> = {};
    for (const [key, decode] of Object.entries(schema)) {
      const parsed = decode(input[key]);
      if (parsed !== undefined) output[key] = parsed;
    }
    // Only schema-owned fields are published; unknown wire fields are not UI data.
    return output as { [K in keyof S]: ReturnType<S[K]> };
  };
}
export function response<T>(
  decode: Decoder<T>,
  value: unknown,
  subject: string,
  outcomeUnknown = false,
): T {
  try {
    return decode(value);
  } catch {
    throw new DesktopError({
      code: "transport_invalid_response",
      outcomeUnknown,
      message: `${subject}回复无法核查，请重新读取；未采用异常数据。`,
    });
  }
}
