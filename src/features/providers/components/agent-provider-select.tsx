import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { availableProviderModelIds } from "@/features/providers/catalog";
import type { NativeConfig, Provider, ProviderStatus } from "../types";
import { dynamicBindingFor } from "../types";

export function AgentProviderSelect({
  name,
  configId,
  providers,
  status,
  checking,
  unavailable,
  onSelect,
}: {
  name: string;
  configId: string;
  providers: Provider[];
  status: ProviderStatus | null;
  checking: boolean;
  unavailable: boolean;
  onSelect: (config: NativeConfig, providerId: string) => void;
}) {
  if (!["claude", "claude-desktop", "codex", "opencode"].includes(configId))
    return (
      <p className="text-label text-fg-muted">
        {configId === "antigravity" ? "在 Agent 内选择模型" : "暂不支持切换"}
      </p>
    );
  const config = status?.configs.find((item) => item.agentId === configId);
  const route = config?.endpoint
    ? status?.transactions.find(
        (item) =>
          item.agentId === configId &&
          item.state === "applied" &&
          item.routing?.localEndpoint === config.endpoint,
      )
    : undefined;
  const configured = providers.filter((provider) => provider.configured);
  const dynamic =
    status && config ? dynamicBindingFor(status, config) : undefined;
  const native = ["claude", "codex"].includes(configId);
  const current =
    config?.mode === "native" && native
      ? "native"
      : dynamic
        ? dynamic.target.provider.id
        : configured.some((provider) => provider.id === route?.provider?.id)
          ? route!.provider!.id
          : "current";
  return (
    <div className="w-60">
      <Select
        size="sm"
        value={current}
        disabled={checking || unavailable || !config}
        onValueChange={(value) => {
          if (config && value !== "current") onSelect(config, value);
        }}
      >
        <SelectTrigger
          className="w-full"
          aria-label={`${name} Provider`}
          title="切换 Provider，确认后应用"
          placeholder="切换 Provider"
        />
        <SelectContent align="end">
          {current === "current" && (
            <SelectItem value="current" disabled>
              {config?.providerName || "切换 Provider"}
            </SelectItem>
          )}
          {native && <SelectItem value="native">官方原生登录</SelectItem>}
          {configured.map((provider) => (
            <SelectItem
              key={provider.id}
              value={provider.id}
              disabled={!availableProviderModelIds(provider).length}
            >
              {provider.name}
              {!availableProviderModelIds(provider).length
                ? " · 无可用模型"
                : ""}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}
