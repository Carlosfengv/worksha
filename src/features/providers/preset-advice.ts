import type {
  PresetAdvice as ContractAdvice,
  PresetOrigin,
} from "../../../contracts/domain/provider-presets/types";
import { providerCatalog } from "./catalog";
import type { Provider } from "./types";

export type { PresetOrigin };
export type PresetAdvice = ContractAdvice & {
  protocol: Provider["protocol"];
  endpoints: Partial<Record<Provider["protocol"], string>>;
};
export const builtinPresetAdvice: readonly PresetAdvice[] = Object.freeze(
  Object.entries(providerCatalog).map(([presetId, p]) => ({
    origin: { kind: "builtin" as const, presetId },
    sourceName: "内置",
    name: p.name,
    protocol: p.protocol,
    endpoints: p.endpoints,
    model: p.model ?? "",
    documents: p.documents ?? null,
  })),
);
export function presetKey(origin: PresetOrigin) {
  return origin.kind === "builtin"
    ? JSON.stringify(["builtin", origin.presetId])
    : JSON.stringify([
        "external",
        origin.pluginId,
        origin.digest,
        origin.presetId,
      ]);
}
export function presetLabel(advice: PresetAdvice) {
  return advice.origin.kind === "builtin"
    ? advice.name
    : `${advice.name} · 外部 ${advice.sourceName}（${advice.origin.pluginId}）`;
}
const protocol = (value: string): value is Provider["protocol"] =>
  ["responses", "messages", "chat-completions"].includes(value);
export function validatePresetAdvice(value: ContractAdvice): PresetAdvice {
  const label = (s: string) =>
    s.trim() && [...s].length <= 100 && !/\p{Cc}/u.test(s);
  const validEndpoint = (s: string) => {
    const url = new URL(s);
    return (
      s.length <= 2048 &&
      ["http:", "https:"].includes(url.protocol) &&
      !url.username &&
      !url.password &&
      !url.search &&
      !url.hash
    );
  };
  if (
    !label(value.name) ||
    !label(value.sourceName) ||
    !protocol(value.protocol) ||
    [...value.model].length > 200 ||
    /\p{Cc}/u.test(value.model) ||
    Object.keys(value.endpoints).length > 3
  )
    throw Error("供应商预设内容无效。");
  for (const [p, endpoint] of Object.entries(value.endpoints))
    if (!protocol(p) || !validEndpoint(endpoint))
      throw Error("预设服务地址无效。");
  if (value.origin.kind === "builtin") {
    const known = builtinPresetAdvice.find(
      (p) =>
        p.origin.kind === "builtin" &&
        p.origin.presetId === value.origin.presetId,
    );
    if (
      !known ||
      value.name !== known.name ||
      value.protocol !== known.protocol ||
      value.model !== known.model ||
      value.sourceName !== known.sourceName ||
      value.documents !== known.documents ||
      JSON.stringify(Object.entries(value.endpoints).sort()) !==
        JSON.stringify(Object.entries(known.endpoints).sort())
    )
      throw Error("内置预设与当前应用不匹配。");
  } else {
    const { pluginId, digest, presetId } = value.origin;
    if (
      !/^[a-zA-Z0-9._-]{1,128}$/.test(pluginId) ||
      !/^[a-zA-Z0-9._-]{1,64}$/.test(presetId) ||
      !/^[a-f0-9]{64}$/.test(digest) ||
      Object.keys(value.endpoints).length !== 1 ||
      !value.endpoints[value.protocol] ||
      value.documents != null
    )
      throw Error("外部预设身份或范围无效。");
  }
  return value as PresetAdvice;
}
export function presetDefaultEndpoint(
  state: {
    provider: Provider;
    preset: PresetAdvice | null;
  },
  selectedProtocol = state.provider.protocol,
) {
  return state.preset?.endpoints[selectedProtocol];
}
export function presetCredentialLabel(name: string) {
  const encoder = new TextEncoder();
  let prefix = "",
    bytes = encoder.encode(" API Key").length;
  for (const character of name) {
    const size = encoder.encode(character).length;
    if (bytes + size > 200) break;
    prefix += character;
    bytes += size;
  }
  return `${prefix} API Key`;
}
