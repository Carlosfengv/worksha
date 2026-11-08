import { Button } from "@/components/ui/button";
import { domainApi } from "@/features/domain/api";
import { DomainConsentControls } from "@/features/domain/consent-controls";
import type { Status } from "../../../contracts/plugins/sources/types";

export function SourceAccessControls({
  status,
  busy,
  run,
  onConsentChanged,
  readLabel = "读取插件数据",
}: {
  status: Status;
  busy: boolean;
  run: (operation: () => Promise<unknown>) => void;
  onConsentChanged: () => void;
  readLabel?: string;
}) {
  const { manifest, digest, grantRevision } = status;
  const target = { pluginId: manifest.id, digest };
  return (
    <>
      <div className="flex flex-wrap gap-2">
        <Button
          variant="tertiary"
          disabled={busy || !status.runtimeActive}
          onClick={() => run(() => domainApi.authorizeSource(target))}
        >
          选择数据文件
        </Button>
        <Button
          variant="tertiary"
          disabled={busy || !status.sourceAuthorized}
          onClick={() => run(() => domainApi.refreshSource(target))}
        >
          {readLabel}
        </Button>
      </div>
      {status.runtimeActive && manifest.source && (
        <DomainConsentControls
          key={`${digest}:${grantRevision}`}
          pluginId={manifest.id}
          digest={digest}
          sourceId={`${manifest.id}:${manifest.source.id}`}
          fullFacts
          onChanged={onConsentChanged}
        />
      )}
    </>
  );
}
