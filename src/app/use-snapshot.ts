import { appApi } from "@/app/api";
import { usePollingResource } from "@/shared/hooks/use-polling-resource";

const loadSnapshot = () => appApi.snapshot();

export function useSnapshot() {
  const { data, error, loading, invalidate } = usePollingResource(
    "snapshot",
    loadSnapshot,
    true,
    5_000,
  );
  return { data, error, loading, refresh: invalidate };
}
