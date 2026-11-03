import { useCallback, useSyncExternalStore } from "react";
import {
  backendReadEpoch,
  subscribeBackendRecovery,
} from "../lib/backend-recovery";

export function useBackendReadEpoch(enabled = true) {
  const subscribe = useCallback(
    (listener: () => void) =>
      enabled ? subscribeBackendRecovery(listener) : () => {},
    [enabled],
  );
  return useSyncExternalStore(subscribe, backendReadEpoch);
}
