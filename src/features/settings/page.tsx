import type { Snapshot } from "@/app/types";
import {
  DetailList,
  DetailListItem,
  DetailListLabel,
  DetailListValue,
} from "@/components/ui/detail-list";
import {
  SegmentedBar,
  ChartLegend,
  chartSeriesColor,
} from "@/components/ui/chart-primitives";
import { bytes } from "@/features/sessions/display";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { SettingsPreferences } from "./components/settings-preferences";
import type { ReactNode } from "react";
import {
  PageBody,
  PageContent,
  PageLayout,
  PageSidebar,
} from "@/components/ui/page-layout";
import { NavMenu } from "@/components/ui/nav-menu";
import {
  NavItem,
  NavItemLabel,
  NavItemTrigger,
} from "@/components/ui/nav-item";
import { BackendConnection } from "./components/backend-connection";
import { RetentionPreviewPanel } from "./components/retention-preview";
import { RouteArchive } from "./components/route-archive";

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
      {data.settingsWriteProtocol?.storeId && (
        <RouteArchive
          key={data.settingsWriteProtocol.storeId}
          storeId={data.settingsWriteProtocol.storeId}
        />
      )}
      <section className="space-y-3">
        <h2 className="text-title font-medium">待开放能力</h2>
        <p className="text-body text-fg-muted">
          会话清理与恢复尚未开放，当前不会移动或删除原始会话。重置券直接兑换尚未开放；账户中已有的官方入口仍可使用，结果以服务商为准。
        </p>
      </section>
    </div>
  );
}

function SqliteStorage({ storage }: { storage: Snapshot["sqliteStorage"] }) {
  const labels = {
    configuration: "配置数据库",
    observation: "观测数据库",
  } as const;
  const items = Object.entries(labels).map(([id, label], index) => ({
    id,
    label,
    value: storage?.[id as keyof typeof labels] ?? null,
    color: chartSeriesColor(id, { colorIndex: index === 0 ? 1 : 2 }),
  }));
  const measured = items.flatMap((item) =>
    item.value == null ? [] : [{ ...item, value: item.value }],
  );
  return (
    <section className="max-w-3xl space-y-3" aria-label="本地 SQLite 存储分布">
      <h2 className="text-title font-medium">本地 SQLite 存储分布</h2>
      {measured.length > 0 && (
        <SegmentedBar
          mode="distribution"
          segments={measured}
          valueText="已测量的 SQLite 文件大小"
        />
      )}
      <ChartLegend
        items={items.map((item) => ({
          ...item,
          value: item.value == null ? "未知" : bytes(item.value),
        }))}
      />
      <p className="text-label text-fg-muted">
        统计本机配置与观测数据库及其 WAL、SHM 文件。
      </p>
    </section>
  );
}

const sections = [
  ["preferences", "外观与捕获偏好"],
  ["storage", "存储与留存预览"],
  ["iam", "账号管理"],
  ["connection", "本机连接诊断"],
] as const;

export function SettingsPage({
  data,
  refresh,
  section,
  accountSettings,
}: {
  data: Snapshot | null;
  refresh: () => Promise<void>;
  section?: string;
  accountSettings?: ReactNode;
}) {
  const visible = sections.filter(([id]) => id !== "iam" || accountSettings);
  const current = visible.find(([id]) => section === id)?.[0] ?? "preferences";
  return (
    <PageLayout size="full" gutter="none">
      <PageSidebar width="216px" aria-label="系统设置分区" className="p-2">
        <NavMenu activeValue={current} aria-label="系统设置分区">
          {visible.map(([id, title]) => (
            <NavItem key={id} value={id}>
              <NavItemTrigger href={`#/settings?tab=${id}`}>
                <NavItemLabel>{title}</NavItemLabel>
              </NavItemTrigger>
            </NavItem>
          ))}
        </NavMenu>
      </PageSidebar>
      <PageContent>
        <PageBody className="mx-0 space-y-5 p-4">
          {current === "preferences" && (
            <SettingsPreferences
              value={data?.settings}
              revision={data?.settingsRevision}
              protocol={data?.settingsWriteProtocol}
              refresh={refresh}
            />
          )}
          {current === "storage" && (
            <>
              <SqliteStorage storage={data?.sqliteStorage} />
              <RetentionPreviewPanel
                protocol={data?.settingsWriteProtocol}
                revision={data?.settingsRevision}
                days={data?.settings.retentionDays}
              />
            </>
          )}
          {current === "iam" && accountSettings}
          {current === "connection" && (
            <>
              <BackendConnection />
              {data ? (
                <SettingsForm data={data} />
              ) : (
                <ConnectionEmpty title="本地数据暂不可用" icon="settings" />
              )}
            </>
          )}
        </PageBody>
      </PageContent>
    </PageLayout>
  );
}
