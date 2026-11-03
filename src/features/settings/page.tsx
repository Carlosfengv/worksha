import type { Snapshot } from "@/app/types";
import {
  DetailList,
  DetailListItem,
  DetailListLabel,
  DetailListValue,
} from "@/components/ui/detail-list";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { SettingsPreferences } from "./components/settings-preferences";
import { GatewayPanel } from "./gateway-panel";
import { BackendConnection } from "./components/backend-connection";
import { RetentionPreviewPanel } from "./components/retention-preview";

function SettingsForm({ data }: { data: Snapshot }) {
  return (
    <div className="max-w-3xl space-y-6">
      <section className="space-y-3">
        <h2 className="text-title font-medium">本地数据</h2>
        <DetailList>
          <DetailListItem>
            <DetailListLabel>访问范围</DetailListLabel>
            <DetailListValue>仅本机</DetailListValue>
          </DetailListItem>
          <DetailListItem>
            <DetailListLabel>数据位置</DetailListLabel>
            <DetailListValue>{data.databasePath}</DetailListValue>
          </DetailListItem>
          <DetailListItem>
            <DetailListLabel>版本</DetailListLabel>
            <DetailListValue>{data.version}</DetailListValue>
          </DetailListItem>
        </DetailList>
      </section>
      <section className="space-y-3">
        <h2 className="text-title font-medium">待开放能力</h2>
        <p className="text-body text-fg-muted">
          会话清理与恢复尚未开放，当前不会移动或删除原始会话。重置券直接兑换尚未开放；账户中已有的官方入口仍可使用，结果以服务商为准。
        </p>
      </section>
    </div>
  );
}

export function SettingsPage({
  data,
  refresh,
}: {
  data: Snapshot | null;
  refresh: () => Promise<void>;
}) {
  return (
    <div className="max-w-3xl space-y-6">
      <BackendConnection />
      <SettingsPreferences
        value={data?.settings}
        revision={data?.settingsRevision}
        protocol={data?.settingsWriteProtocol}
        refresh={refresh}
      />
      <GatewayPanel
        providers={data?.providers ?? []}
        storeId={data?.settingsWriteProtocol?.storeId}
      />
      <RetentionPreviewPanel
        protocol={data?.settingsWriteProtocol}
        revision={data?.settingsRevision}
        days={data?.settings.retentionDays}
      />
      {data ? (
        <SettingsForm data={data} />
      ) : (
        <ConnectionEmpty
          title="设置暂不可用"
          description="连接桌面应用的本地数据服务后重试。"
          icon="settings"
        />
      )}
    </div>
  );
}
