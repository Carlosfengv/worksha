import {
  defaultEndpoint,
  providerCatalog,
  providerVendor,
  usesDefaultEndpoint,
} from "./catalog";
import type { Provider } from "./types";
import {
  builtinPresetAdvice,
  presetDefaultEndpoint,
  presetCredentialLabel,
  type PresetAdvice,
} from "./preset-advice";

export interface ProviderEditorDraft {
  preset: PresetAdvice | null;
  provider: Provider & { vendor: string };
  vendorChosen: boolean;
  endpointMode: "default" | "custom";
  customEndpoint: string;
  catalog: string;
  label: string;
  mode: "catalog" | "automatic";
  advanced: boolean;
  manualModel: boolean;
  showModel: boolean;
}
export type ProviderDraftAction =
  | { type: "preset"; value: PresetAdvice }
  | { type: "vendor"; value: string }
  | { type: "protocol"; value: Provider["protocol"] }
  | { type: "endpoint-source"; value: "default" | "custom" }
  | {
      type:
        "name" | "endpoint" | "model" | "select-model" | "label" | "catalog";
      value: string;
    }
  | { type: "mode"; value: ProviderEditorDraft["mode"] }
  | { type: "manual-model" | "show-model"; value: boolean }
  | { type: "advanced" }
  | { type: "provider"; value: Provider };

export function providerEditorDraft(provider: Provider): ProviderEditorDraft {
  const defaults = usesDefaultEndpoint(provider);
  const preset =
    builtinPresetAdvice.find(
      (p) =>
        p.origin.kind === "builtin" &&
        p.origin.presetId === providerVendor(provider),
    ) ?? null;
  const model =
    provider.model || (provider.configured ? "" : (preset?.model ?? ""));
  return {
    preset,
    provider: {
      ...provider,
      vendor: providerVendor(provider),
      model,
      models: [...provider.models],
    },
    vendorChosen: provider.configured || providerVendor(provider) !== "custom",
    endpointMode: defaults ? "default" : "custom",
    customEndpoint: defaults ? "" : provider.endpoint,
    catalog: provider.models.length ? provider.models.join(", ") : model,
    label: provider.credentialLabel ?? `${provider.name || "新连接"} API Key`,
    mode: "catalog",
    advanced: false,
    manualModel: false,
    showModel: false,
  };
}
export function reduceProviderDraft(
  state: ProviderEditorDraft,
  action: ProviderDraftAction,
): ProviderEditorDraft {
  const draft = state.provider;
  switch (action.type) {
    case "vendor": {
      const template = builtinPresetAdvice.find(
        (p) =>
          p.origin.kind === "builtin" && p.origin.presetId === action.value,
      );
      if (!template) return state;
      return reduceProviderDraft(state, { type: "preset", value: template });
    }
    case "preset": {
      const template = action.value;
      const plan =
        template.origin.kind === "builtin" &&
        template.origin.presetId === "minimax-plan";
      const useDefault =
        plan || state.endpointMode === "default" || !draft.endpoint;
      const changedCredentialScope = plan !== (draft.vendor === "minimax-plan");
      const model =
        !draft.model && !state.catalog.trim() && !draft.models.length
          ? template.model
          : draft.model;
      return {
        ...state,
        preset: template,
        vendorChosen: true,
        provider: {
          ...draft,
          vendor:
            template.origin.kind === "builtin"
              ? template.origin.presetId
              : "custom",
          name:
            !draft.name ||
            draft.name ===
              (state.preset?.name ?? providerCatalog[draft.vendor]?.name)
              ? template.name
              : draft.name,
          protocol: template.protocol,
          endpoint: useDefault
            ? (template.endpoints[template.protocol] ?? "")
            : draft.endpoint,
          ...(changedCredentialScope
            ? { credentialRef: null, credentialLabel: null, modelCatalog: null }
            : {}),
          model,
        },
        catalog:
          model && !draft.model && !state.catalog.trim()
            ? model
            : state.catalog,
        endpointMode: useDefault
          ? template.endpoints[template.protocol]
            ? "default"
            : "custom"
          : state.endpointMode,
        label:
          draft.credentialRef && !changedCredentialScope
            ? state.label
            : presetCredentialLabel(template.name),
      };
    }
    case "protocol": {
      const endpoint = presetDefaultEndpoint(state, action.value);
      return {
        ...state,
        provider: {
          ...draft,
          protocol: action.value,
          endpoint:
            state.endpointMode === "default"
              ? (endpoint ?? "")
              : draft.endpoint,
        },
        endpointMode:
          state.endpointMode === "default" && !endpoint
            ? "custom"
            : state.endpointMode,
      };
    }
    case "endpoint-source":
      return {
        ...state,
        endpointMode: action.value,
        customEndpoint:
          action.value === "default" ? draft.endpoint : state.customEndpoint,
        provider: {
          ...draft,
          endpoint:
            action.value === "default"
              ? (presetDefaultEndpoint(state) ?? defaultEndpoint(draft) ?? "")
              : state.customEndpoint || draft.endpoint,
        },
      };
    case "endpoint":
      return {
        ...state,
        customEndpoint: action.value,
        endpointMode: "custom",
        provider: { ...draft, endpoint: action.value },
      };
    case "name":
      return { ...state, provider: { ...draft, name: action.value } };
    case "model":
      return { ...state, provider: { ...draft, model: action.value } };
    case "select-model":
      return {
        ...state,
        provider: {
          ...draft,
          model: action.value,
          disabledModels: (draft.disabledModels ?? []).filter(
            (id) => id !== action.value,
          ),
        },
      };
    case "label":
      return { ...state, label: action.value };
    case "catalog":
      return { ...state, catalog: action.value };
    case "mode":
      return { ...state, mode: action.value };
    case "manual-model":
      return { ...state, manualModel: action.value };
    case "show-model":
      return { ...state, showModel: action.value };
    case "advanced":
      return {
        ...state,
        mode: "catalog",
        advanced: state.mode === "automatic" || !state.advanced,
      };
    case "provider":
      return {
        ...state,
        provider: { ...action.value, vendor: providerVendor(action.value) },
      };
  }
}
