import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectSeparator,
  SelectTrigger,
} from "@/components/ui/select";
import { availableProviderModelIds } from "@/features/providers/catalog";
import type { Provider } from "@/features/providers/types";
import type { ModelService } from "../types";

type ConnectionChoice =
  { kind: "provider" | "service"; id: string } | { kind: "native" };

export function AgentConnectionPicker({
  name,
  providers,
  services,
  emptyServices,
  native = false,
  disabled,
  onSelect,
}: {
  name: string;
  providers: Provider[];
  services: ModelService[] | undefined;
  emptyServices: string;
  native?: boolean;
  disabled?: boolean;
  onSelect: (choice: ConnectionChoice) => void;
}) {
  const configured = providers.filter((provider) => provider.configured);
  return (
    <Select
      value=""
      size="sm"
      disabled={disabled}
      onValueChange={(value) => {
        if (value === "native" && native) onSelect({ kind: "native" });
        else if (value.startsWith("provider:"))
          onSelect({ kind: "provider", id: value.slice(9) });
        else if (value.startsWith("service:"))
          onSelect({ kind: "service", id: value.slice(8) });
      }}
    >
      <SelectTrigger aria-label={`${name} 模型连接`} placeholder="切换" />
      <SelectContent>
        <SelectGroup aria-label="Provider">
          <SelectLabel>Provider</SelectLabel>
          {!configured.length && (
            <SelectItem value="empty-providers" disabled>
              暂无已配置 Provider
            </SelectItem>
          )}
          {configured.map((provider) => {
            const hasModels = availableProviderModelIds(provider).length > 0;
            return (
              <SelectItem
                key={provider.id}
                value={`provider:${provider.id}`}
                disabled={!hasModels}
              >
                {provider.name}
                {!hasModels && " · 无可用模型"}
              </SelectItem>
            );
          })}
        </SelectGroup>
        <SelectSeparator />
        <SelectGroup aria-label="模型服务">
          <SelectLabel>模型服务</SelectLabel>
          {services?.length ? (
            services.map((service) => (
              <SelectItem key={service.id} value={`service:${service.id}`}>
                {service.name}
              </SelectItem>
            ))
          ) : (
            <SelectItem value="empty-services" disabled>
              {emptyServices}
            </SelectItem>
          )}
        </SelectGroup>
        {native && (
          <>
            <SelectSeparator />
            <SelectItem value="native">官方原生登录</SelectItem>
          </>
        )}
      </SelectContent>
    </Select>
  );
}
