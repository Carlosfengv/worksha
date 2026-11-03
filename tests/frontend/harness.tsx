import { StrictMode, useCallback, useState } from "react";
import { createRoot } from "react-dom/client";
import { appApi } from "@/app/api";
import type { Snapshot } from "@/app/types";
import { useSnapshot } from "@/app/use-snapshot";
import { useNativePage } from "@/features/sessions/hooks/use-native-page";
import type { NativePage } from "@/features/sessions/types";
import { usePollingResource } from "@/shared/hooks/use-polling-resource";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";

type Page = NativePage<{ id: string }>;
type Read = {
  key: string;
  cursor?: string;
  resolve: (value: Page | Snapshot) => void;
  reject: (error: Error) => void;
};
declare global {
  interface Window {
    hookReads: Read[];
    hookThrows?: boolean;
    hookAfterMutation?: () => Promise<void>;
    hookRecover: () => void;
    hookOldActions?: {
      next: () => Promise<void>;
      previous: () => Promise<void>;
      reload: () => Promise<void>;
      invalidate: () => Promise<void>;
    };
  }
}
window.hookReads = [];
window.hookRecover = notifyBackendRecovery;
function read(key: string, cursor?: string) {
  if (window.hookThrows) throw new Error("synchronous loader failure");
  return new Promise<Page | Snapshot>((resolve, reject) =>
    window.hookReads.push({ key, cursor, resolve, reject }),
  );
}
appApi.snapshot = () =>
  read("snapshot").then((value) => {
    if ("version" in value) return value;
    throw new Error("Invalid snapshot fixture");
  });
const readPage = (key: string, cursor?: string) =>
  read(key, cursor).then((value) => {
    if ("items" in value) return value;
    throw new Error("Invalid page fixture");
  });
function NativeProbe({ query }: { query: string }) {
  const [bounded, setBounded] = useState(
    new URLSearchParams(location.search).get("retention") === "page",
  );
  const [completed, setCompleted] = useState(0);
  const loader = useCallback(
    (cursor?: string) => readPage(query, cursor),
    [query],
  );
  const resource = useNativePage(query, loader, bounded ? "page" : "append");
  return (
    <>
      <output data-testid="items">
        {resource.data?.items.map((item) => item.id).join(",") ?? "empty"}
      </output>
      <output data-testid="error">{resource.error}</output>
      <output data-testid="loading">{String(resource.loading)}</output>
      <output data-testid="cursor">{resource.cursor ?? "first"}</output>
      <output data-testid="trail">{resource.previousCursors.length}</output>
      <output data-testid="retained">{resource.data?.items.length ?? 0}</output>
      <button onClick={() => void resource.loadPrevious()}>上一页</button>
      <button onClick={() => setBounded((value) => !value)}>
        切换保留方式
      </button>
      <button onClick={() => void resource.loadMore()}>更多</button>
      <button
        onClick={() => {
          window.hookOldActions = {
            next: resource.loadMore,
            previous: resource.loadPrevious,
            reload: resource.reload,
            invalidate: resource.invalidate,
          };
        }}
      >
        保留本页动作
      </button>
      <button onClick={() => void resource.reload()}>重新读取</button>
      <button
        onClick={() => {
          window.hookAfterMutation = resource.invalidate;
        }}
      >
        保留变更回调
      </button>
      <button
        onClick={async () => {
          await resource.invalidate();
          setCompleted((n) => n + 1);
        }}
      >
        变更后刷新
      </button>
      <output data-testid="completed">{completed}</output>
    </>
  );
}
function ResourceProbe({ query }: { query: string }) {
  const [completed, setCompleted] = useState(0);
  const loader = useCallback(() => {
    if (window.hookThrows) throw new Error("synchronous loader failure");
    return readPage(query);
  }, [query]);
  const resource = usePollingResource(query, loader);
  return (
    <>
      <output data-testid="items">
        {resource.data?.items.map((item) => item.id).join(",") ?? "empty"}
      </output>
      <output data-testid="error">{resource.error}</output>
      <output data-testid="loading">{String(resource.loading)}</output>
      <button onClick={resource.reload}>重新读取</button>
      <button
        onClick={() => {
          window.hookAfterMutation = resource.invalidate;
        }}
      >
        保留变更回调
      </button>
      <button
        onClick={async () => {
          await resource.invalidate();
          setCompleted((n) => n + 1);
        }}
      >
        变更后刷新
      </button>
      <output data-testid="completed">{completed}</output>
    </>
  );
}
function SnapshotProbe() {
  const snapshot = useSnapshot();
  const [completed, setCompleted] = useState(false);
  return (
    <>
      <output data-testid="snapshot">
        {snapshot.data?.version ?? "empty"}
      </output>
      <output data-testid="loading">{String(snapshot.loading)}</output>
      <output data-testid="error">{snapshot.error}</output>
      <output data-testid="completed">{String(completed)}</output>
      <button
        onClick={async () => {
          await snapshot.refresh();
          setCompleted(true);
        }}
      >
        刷新
      </button>
    </>
  );
}
function Harness() {
  const [query, setQuery] = useState("first");
  const [visible, setVisible] = useState(true);
  const mode = new URLSearchParams(location.search).get("mode");
  return (
    <>
      <button onClick={() => setQuery("second")}>切换筛选</button>
      <button onClick={() => setVisible((value) => !value)}>切换挂载</button>
      {visible &&
        (mode === "snapshot" ? (
          <SnapshotProbe />
        ) : mode === "resource" ? (
          <ResourceProbe query={query} />
        ) : (
          <NativeProbe query={query} />
        ))}
    </>
  );
}
const root = document.getElementById("root");
if (!root) throw new Error("Missing harness root");
const strict = new URLSearchParams(location.search).has("strict");
createRoot(root).render(
  strict ? (
    <StrictMode>
      <Harness />
    </StrictMode>
  ) : (
    <Harness />
  ),
);
