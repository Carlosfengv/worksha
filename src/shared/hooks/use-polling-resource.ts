import { useCallback, useEffect, useRef, useState } from "react";
import { subscribeBackendRecovery } from "../lib/backend-recovery";

export function usePollingResource<T>(
  key: string,
  loader: () => Promise<T>,
  enabled = true,
  interval = 10_000,
) {
  const [state, setState] = useState<{
    key: string;
    data: T | null;
    error: string | null;
    loading: boolean;
  }>({ key, data: null, error: null, loading: enabled });
  const control = useRef<{
    key: string;
    reload: () => Promise<void>;
    invalidate: () => Promise<void>;
  } | null>(null);
  const reload = useCallback(
    () =>
      enabled && control.current?.key === key
        ? control.current.reload()
        : Promise.resolve(),
    [key, enabled],
  );
  const invalidate = useCallback(
    () =>
      enabled && control.current?.key === key
        ? control.current.invalidate()
        : Promise.resolve(),
    [key, enabled],
  );
  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    let pending: Promise<void> | null = null;
    let mutation = 0;
    let completedMutation = -1;
    const load = (background = false): Promise<void> => {
      if (!alive) return Promise.resolve();
      if (!background)
        setState((old) => ({
          key,
          data: old.key === key ? old.data : null,
          error: null,
          loading: true,
        }));
      if (pending) return pending;
      let startedMutation = -1;
      // Defer the loader so synchronous throws settle through the same path,
      // and mutations in the same turn share a read that starts after them.
      const request = Promise.resolve()
        .then(() => {
          if (!alive) return;
          startedMutation = mutation;
          return loader().then((data) => {
            if (alive && startedMutation === mutation)
              setState({ key, data, error: null, loading: false });
          });
        })
        .catch((reason: unknown) => {
          if (alive && startedMutation === mutation)
            setState((old) => ({
              key,
              data: old.key === key ? old.data : null,
              error: reason instanceof Error ? reason.message : String(reason),
              loading: false,
            }));
        })
        .finally(() => {
          completedMutation = startedMutation;
          pending = null;
        });
      pending = request;
      return request;
    };
    const instance = {
      key,
      reload: () => load(),
      invalidate: async () => {
        const required = ++mutation;
        if (pending) await load();
        if (!alive || completedMutation >= required) return;
        await load();
      },
    };
    control.current = instance;
    const unsubscribe = subscribeBackendRecovery(
      () => void instance.invalidate(),
    );
    void load();
    const poll = () => {
      if (document.visibilityState === "visible") void load(true);
    };
    const timer = window.setInterval(poll, interval);
    document.addEventListener("visibilitychange", poll);
    return () => {
      alive = false;
      unsubscribe();
      if (control.current === instance) control.current = null;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", poll);
    };
  }, [key, loader, enabled, interval]);
  return {
    data: enabled && state.key === key ? state.data : null,
    error: enabled && state.key === key ? state.error : null,
    loading: enabled && (state.key !== key || state.loading),
    reload,
    invalidate,
  };
}
