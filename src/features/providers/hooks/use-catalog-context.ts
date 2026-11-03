import {
  useCallback,
  useLayoutEffect,
  useRef,
  useSyncExternalStore,
} from "react";
import {
  backendReadEpoch,
  subscribeBackendRecovery,
} from "@/shared/lib/backend-recovery";

export interface CatalogContext {
  readonly storeId: string;
  readonly epoch: symbol;
  readonly generation: number;
}

// A lease belongs to one mounted database view and one confirmed backend epoch.
// Checking the synchronous epoch also covers recovery and reply in the same turn.
export function useCatalogContext(storeId: string | undefined) {
  const epoch = useSyncExternalStore(
    subscribeBackendRecovery,
    backendReadEpoch,
  );
  const current = useRef({ storeId, generation: 0, mounted: false });
  useLayoutEffect(() => {
    current.current = {
      storeId,
      generation: current.current.generation + 1,
      mounted: true,
    };
    return () => {
      current.current.mounted = false;
      current.current.generation++;
    };
  }, [storeId]);
  const isCurrent = useCallback(
    (source: CatalogContext | null) =>
      !!source &&
      source.storeId === storeId &&
      current.current.mounted &&
      current.current.storeId === source.storeId &&
      current.current.generation === source.generation &&
      backendReadEpoch() === source.epoch,
    [storeId],
  );
  const capture = useCallback((): CatalogContext | null => {
    if (
      !storeId ||
      !current.current.mounted ||
      current.current.storeId !== storeId
    )
      return null;
    return {
      storeId,
      epoch: backendReadEpoch(),
      generation: current.current.generation,
    };
  }, [storeId]);
  return { storeId, epoch, capture, isCurrent };
}
