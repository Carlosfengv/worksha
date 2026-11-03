import { useMemo, useSyncExternalStore } from "react";
import type { ReadResource, ReadResourceFamily } from "../lib/read-resource";

const disabled = { data: null, error: null, loading: false };

export function useSharedRead<T>(resource: ReadResource<T>, enabled = true) {
  const lease = useMemo(() => {
    let subscribers = 0;
    return {
      getSnapshot: () => (enabled ? resource.getSnapshot() : disabled),
      subscribe: (listener: () => void) => {
        if (!enabled) return () => {};
        subscribers += 1;
        const unsubscribe = resource.subscribe(listener);
        let active = true;
        return () => {
          if (!active) return;
          active = false;
          subscribers -= 1;
          unsubscribe();
        };
      },
      reload: () => (subscribers ? resource.reload() : Promise.resolve()),
      invalidate: () =>
        subscribers ? resource.invalidate() : Promise.resolve(),
    };
  }, [resource, enabled]);
  const snapshot = useSyncExternalStore(lease.subscribe, lease.getSnapshot);
  return { ...snapshot, reload: lease.reload, invalidate: lease.invalidate };
}

export function useSharedQuery<P, T>(
  family: ReadResourceFamily<P, T>,
  parameters: P,
  enabled = true,
) {
  const key = family.key(parameters);
  const resource = useMemo(() => family.bind(key), [family, key]);
  return useSharedRead(resource, enabled);
}
