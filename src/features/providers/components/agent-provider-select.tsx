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
  const runtime = status?.runtimes.find((item) => item.agentId === configId);
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
  const reason = unavailable
    ? "配置读取失败，请重新读取"
    : checking
      ? "正在读取配置…"
      : !config
        ? "配置状态未知"
        : config.error
          ? "配置读取未完成"
          : !runtime?.supported
            ? (runtime?.reason ?? "当前版本暂不支持切换")
            : !config.revision
              ? "配置版本未知，请重新读取"
              : null;
  return (
    <div className="w-60 space-y-1">
      <Select
        size="sm"
        value={current}
        disabled={!!reason}
        onValueChange={(value) => {
          if (config && value !== "current") onSelect(config, value);
        }}
      >
        <SelectTrigger
          className="w-full"
          aria-label={`${name} Provider`}
          title={reason ?? "选择 Provider，确认后应用"}
          placeholder={reason ?? "选择 Provider"}
        />
        <SelectContent align="end">
          {current === "current" && (
            <SelectItem value="current" disabled>
              {reason ?? config?.providerName ?? "连接待确认"}
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
      <p
        className="truncate text-label text-fg-subtle"
        title={reason ?? config?.model ?? undefined}
      >
        {reason ??
          dynamic?.target.provider.model ??
          config?.model ??
          "模型由 Agent 管理"}
      </p>
    </div>
  );
}
