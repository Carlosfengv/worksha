import {
  Alert,
  AlertAction,
  AlertDescription,
  AlertTitle,
} from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import type { Profile } from "../../../contracts/plugins/builtin/types";

export function BuiltinCompositionNotice({
  composition,
  className,
}: {
  composition?: { profile: Profile; problem: string | null };
  className?: string;
}) {
  if (
    !composition ||
    (composition.profile === "desktop" && !composition.problem)
  )
    return null;
  const recovery = composition.profile === "recovery" || !!composition.problem;
  return (
    <div className={className}>
      <Alert aria-label="功能可用状态">
        <AlertTitle>
          {composition.problem
            ? "部分功能需要恢复"
            : recovery
              ? "应用功能已暂停"
              : "自动更新已暂停"}
        </AlertTitle>
        <AlertDescription>
          {recovery
            ? "部分功能暂时无法使用。可到插件中心恢复应用功能；来源读取仍需单独授权。"
            : "会话和活动等数据的自动更新已暂停，其他功能入口继续可用。可到插件中心检查并恢复更新。"}
        </AlertDescription>
        <AlertAction>
          <Button
            variant="secondary"
            onClick={() => {
              window.location.hash = "/plugins";
            }}
          >
            打开插件中心
          </Button>
        </AlertAction>
      </Alert>
    </div>
  );
}
