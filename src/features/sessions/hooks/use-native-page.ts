import type { NativePage } from "@/features/sessions/types";
import { appendUniqueById } from "../session-collections";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  backendReadEpoch,
  subscribeBackendRecovery,
} from "@/shared/lib/backend-recovery";
import { useBackendReadEpoch } from "@/shared/hooks/use-backend-read-epoch";
import { DesktopError } from "@/shared/lib/desktop-command";

const EMPTY_TRAIL: readonly (string | undefined)[] = [];

export function useNativePage<T extends { id: string }>(
  queryKey: string,
  loader: (cursor?: string) => Promise<NativePage<T>>,
  mode: "append" | "page" = "append",
) {
  const key = JSON.stringify([queryKey, mode]);
  const readEpoch = useBackendReadEpoch();
  const [state, setState] = useState<{
    key: string;
    data: NativePage<T> | null;
    error: string | null;
    loading: boolean;
    epoch: symbol;
    cursor?: string;
    previous: readonly (string | undefined)[];
    scopeInvalid: boolean;
  }>({
    key,
    data: null,
    error: null,
    loading: true,
    epoch: readEpoch,
    previous: EMPTY_TRAIL,
    scopeInvalid: false,
  });
  const current = useRef(0);
  const lifecycle = useRef(0);
  const mounted = useRef(false);
  const paged = useRef(false);
  const invalidating = useRef(0);
  const active = useRef<{ key: string; loader: typeof loader } | null>(null);
  const pending = useRef<{
    cursor?: string;
    promise: Promise<void>;
  } | null>(null);
  const load = useCallback(
    (
      cursor?: string,
      background = false,
      direction: "first" | "next" | "previous" = "first",
    ): Promise<void> => {
      if (
        !mounted.current ||
        active.current?.key !== key ||
        active.current.loader !== loader
      )
        return Promise.resolve();
      if (pending.current && (background || pending.current.cursor === cursor))
        return pending.current.promise;
      const sequence = ++current.current;
      const epoch = backendReadEpoch();
      // Stop first-page polling as soon as pagination starts, even if it fails.
      // The backend still completes older IPC reads; only their publication is cancelled.
      paged.current = Boolean(cursor);
      if (!background)
        setState((old) => ({
          ...old,
          key,
          epoch,
          data: old.key === key && old.epoch === epoch ? old.data : null,
          previous:
            old.key === key && old.epoch === epoch ? old.previous : EMPTY_TRAIL,
          cursor:
            old.key === key && old.epoch === epoch ? old.cursor : undefined,
          scopeInvalid:
            old.key === key && old.epoch === epoch && old.scopeInvalid,
          loading: true,
          error: null,
        }));
      const promise: Promise<void> = (async () => {
        try {
          const response = await loader(cursor);
          if (
            !mounted.current ||
            current.current !== sequence ||
            epoch !== backendReadEpoch()
          )
            return;
          setState((old) => {
            const owned = old.key === key && old.epoch === epoch;
            const disconnected =
              mode === "page" && response.coverage === "not_connected";
            const previous =
              mode === "append" && cursor && owned ? old.data : null;
            let trail = EMPTY_TRAIL;
            if (mode === "page" && owned && old.data) {
              if (direction === "next")
                trail = [...old.previous, old.cursor].slice(-100);
              else if (direction === "previous")
                trail = old.previous.slice(0, -1);
            }
            return {
              key,
              epoch,
              cursor,
              previous: disconnected ? EMPTY_TRAIL : trail,
              scopeInvalid: disconnected,
              loading: false,
              error: null,
              data: previous
                ? {
                    ...response,
                    items: appendUniqueById(previous.items, response.items),
                  }
                : { ...response, items: appendUniqueById([], response.items) },
            };
          });
        } catch (reason) {
          const rejected =
            mode === "page" &&
            reason instanceof DesktopError &&
            ["unauthorized", "not_found", "stale_snapshot"].includes(
              reason.code,
            );
          if (
            mounted.current &&
            current.current === sequence &&
            epoch === backendReadEpoch()
          )
            setState((old) => ({
              ...old,
              key,
              epoch,
              data: rejected
                ? null
                : old.key === key && old.epoch === epoch
                  ? old.data
                  : null,
              previous: rejected
                ? EMPTY_TRAIL
                : old.key === key && old.epoch === epoch
                  ? old.previous
                  : EMPTY_TRAIL,
              scopeInvalid:
                rejected ||
                (old.key === key && old.epoch === epoch && old.scopeInvalid),
              loading: false,
              error: reason instanceof Error ? reason.message : String(reason),
            }));
        }
      })().finally(() => {
        // Publication can be invalidated while this read is still running.
        // Only its own pending slot may be released, even after a sync throw.
        if (pending.current?.promise === promise) pending.current = null;
      });
      pending.current = { cursor, promise };
      return promise;
    },
    [key, loader, mode],
  );
  useEffect(() => {
    mounted.current = true;
    paged.current = false;
    invalidating.current = 0;
    active.current = { key, loader };
    void load();
    const poll = () => {
      if (!paged.current && document.visibilityState === "visible")
        void load(undefined, true);
    };
    const timer = window.setInterval(poll, 5000);
    document.addEventListener("visibilitychange", poll);
    return () => {
      mounted.current = false;
      lifecycle.current += 1;
      current.current += 1;
      pending.current = null;
      active.current = null;
      invalidating.current = 0;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", poll);
    };
  }, [load, key, loader]);
  const invalidate = useCallback(async () => {
    if (
      !mounted.current ||
      active.current?.key !== key ||
      active.current.loader !== loader
    )
      return;
    const generation = lifecycle.current;
    current.current += 1;
    invalidating.current += 1;
    setState((old) => ({
      ...old,
      key,
      epoch: backendReadEpoch(),
      data:
        old.key === key && old.epoch === backendReadEpoch() ? old.data : null,
      previous:
        old.key === key && old.epoch === backendReadEpoch()
          ? old.previous
          : EMPTY_TRAIL,
      scopeInvalid:
        old.key === key && old.epoch === backendReadEpoch() && old.scopeInvalid,
      loading: true,
      error: null,
    }));
    try {
      if (pending.current) await pending.current.promise;
      if (!mounted.current || lifecycle.current !== generation) return;
      await load();
    } finally {
      if (lifecycle.current === generation) invalidating.current -= 1;
    }
  }, [load, key, loader]);
  useEffect(
    () => subscribeBackendRecovery(() => void invalidate()),
    [invalidate],
  );
  const scopedInvalidate = useCallback(
    () => (readEpoch === backendReadEpoch() ? invalidate() : Promise.resolve()),
    [invalidate, readEpoch],
  );
  return {
    data: state.key === key && state.epoch === readEpoch ? state.data : null,
    error: state.key === key && state.epoch === readEpoch ? state.error : null,
    scopeInvalid:
      state.key === key && state.epoch === readEpoch && state.scopeInvalid,
    loading: state.key !== key || state.epoch !== readEpoch || state.loading,
    cursor:
      state.key === key && state.epoch === readEpoch ? state.cursor : undefined,
    previousCursors:
      state.key === key && state.epoch === readEpoch
        ? state.previous
        : EMPTY_TRAIL,
    reload: () =>
      readEpoch === backendReadEpoch() ? load() : Promise.resolve(),
    invalidate: scopedInvalidate,
    loadMore: () =>
      !invalidating.current &&
      readEpoch === backendReadEpoch() &&
      state.key === key &&
      state.epoch === readEpoch &&
      state.data?.nextCursor
        ? load(state.data.nextCursor, false, "next")
        : Promise.resolve(),
    loadPrevious: () =>
      !invalidating.current &&
      readEpoch === backendReadEpoch() &&
      state.key === key &&
      state.epoch === readEpoch &&
      state.previous.length
        ? load(state.previous[state.previous.length - 1], false, "previous")
        : Promise.resolve(),
  };
}
