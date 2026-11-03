import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import {
  capabilityAccessEpoch,
  subscribeCapabilityAccess,
} from "@/shared/lib/capability-access";
import { providersApi, providerSourceApi } from "../api";
import { catalogAuthority } from "../catalog-authority";
import { message } from "../display";
import { catalogKey, type CatalogState } from "../model-inventory";
import type { Provider } from "../types";
import { useCatalogContext, type CatalogContext } from "./use-catalog-context";

type Job = { source: CatalogContext; access: symbol; generation: number };
type Entry = CatalogState & Job & { authority?: string };
const LEASE_MS = 5000;
const unavailable =
  "目录权限尚未重新核查，已撤下读取结果；已登记模型仍保留，请重新读取";
export function useModelCatalogs(
  providers: Provider[],
  storeId: string | undefined,
  active = true,
) {
  const { epoch, capture, isCurrent } = useCatalogContext(storeId);
  const access = useSyncExternalStore(
    subscribeCapabilityAccess,
    capabilityAccessEpoch,
  );
  const [entries, setEntries] = useState<Record<string, Entry>>({});
  const entriesRef = useRef(entries);
  const deadlines = useRef(new WeakMap<Entry, number>());
  const [pending, setPending] = useState<Job | null>(null);
  const running = useRef<Job | null>(null);
  const generation = useRef(0),
    enabled = useRef(active);
  const connections = useRef(providers);
  const update = useCallback(
    (change: (old: Record<string, Entry>) => Record<string, Entry>) => {
      const next = change(entriesRef.current);
      if (next !== entriesRef.current) {
        entriesRef.current = next;
        setEntries(next);
      }
    },
    [],
  );
  const put = useCallback(
    (key: string, entry: Entry, until = performance.now() + LEASE_MS) => {
      if (entry.data && entry.authority) deadlines.current.set(entry, until);
      update((old) => ({ ...old, [key]: entry }));
    },
    [update],
  );
  const current = useCallback(
    (job: Job | null) =>
      !!job &&
      enabled.current &&
      job.generation === generation.current &&
      job.access === capabilityAccessEpoch() &&
      isCurrent(job.source) &&
      document.visibilityState !== "hidden",
    [isCurrent],
  );
  useLayoutEffect(() => {
    connections.current = providers;
  }, [providers]);
  useLayoutEffect(() => {
    enabled.current = active;
    generation.current++;
    running.current = null;
    setPending(null);
    update(() => ({}));
  }, [active, storeId, epoch, access, update]);
  const keySet = useMemo(() => new Set(providers.map(catalogKey)), [providers]);
  const catalogs: Record<string, CatalogState> = {};
  for (const [key, entry] of Object.entries(entries))
    if (active && keySet.has(key) && current(entry))
      catalogs[key] =
        entry.data && (deadlines.current.get(entry) ?? 0) <= performance.now()
          ? { ...entry, data: undefined, error: unavailable }
          : entry;
  useEffect(() => {
    update((old) => {
      const kept = Object.entries(old).filter(
        ([key, entry]) => keySet.has(key) && current(entry),
      );
      return kept.length === Object.keys(old).length
        ? old
        : Object.fromEntries(kept);
    });
  }, [keySet, current, update]);
  const present = useCallback(
    (key: string, job: Job) =>
      current(job) && connections.current.some((p) => catalogKey(p) === key),
    [current],
  );
  useEffect(() => {
    if (!active || !storeId) return;
    let stopped = false,
      checking = false,
      nextCheck = 0;
    const retire = (key: string, entry: Entry, reason: unknown) => {
      if (present(key, entry) && entriesRef.current[key] === entry)
        put(key, {
          ...entry,
          data: undefined,
          authority: undefined,
          loading: false,
          error: message(reason),
        });
    };
    const verify = async () => {
      checking = true;
      try {
        // Metadata only: no model directories, grants, Vault or inference.
        // One round and one request at a time, only while disclosed data exists.
        for (const [key, entry] of Object.entries(entriesRef.current)) {
          if (stopped) break;
          if (!entry.data || !entry.authority || !present(key, entry)) continue;
          const provider = connections.current.find(
            (p) => catalogKey(p) === key,
          );
          if (!provider) continue;
          try {
            const state = await providerSourceApi.state({
              storeId: entry.source.storeId,
              providerId: provider.id,
              expectedRevision: String(provider.revision),
            });
            if (
              stopped ||
              !present(key, entry) ||
              entriesRef.current[key] !== entry
            )
              continue;
            if (catalogAuthority(state) !== entry.authority)
              throw Error("目录来源或授权已变化，请重新读取；没有改用其他目录");
            // A stalled reply must not renew an already expired disclosure,
            // even if the expiry timer was delayed by a busy render thread.
            if ((deadlines.current.get(entry) ?? 0) <= performance.now()) {
              retire(key, entry, unavailable);
              continue;
            }
            deadlines.current.set(entry, performance.now() + LEASE_MS);
          } catch (reason) {
            if (!stopped) retire(key, entry, reason);
          }
        }
      } finally {
        checking = false;
      }
    };
    const tick = () => {
      const now = performance.now();
      for (const [key, entry] of Object.entries(entriesRef.current))
        if (entry.data && (deadlines.current.get(entry) ?? 0) <= now)
          retire(key, entry, unavailable);
      if (
        !checking &&
        now >= nextCheck &&
        Object.values(entriesRef.current).some((e) => e.data && current(e))
      ) {
        nextCheck = now + 1000;
        void verify();
      }
    };
    const visibility = () => {
      if (document.visibilityState === "hidden") {
        generation.current++;
        running.current = null;
        setPending(null);
        update(() => ({}));
      } else tick();
    };
    const timer = window.setInterval(tick, 250);
    document.addEventListener("visibilitychange", visibility);
    return () => {
      stopped = true;
      clearInterval(timer);
      document.removeEventListener("visibilitychange", visibility);
    };
  }, [active, storeId, epoch, access, current, present, put, update]);
  const readAll = async () => {
    if (
      current(running.current) ||
      !active ||
      document.visibilityState === "hidden"
    )
      return;
    const source = capture();
    if (!source) return;
    const job: Job = {
      source,
      access: capabilityAccessEpoch(),
      generation: generation.current,
    };
    running.current = job;
    setPending(job);
    try {
      // Retain sequential admission to the shared verification capacity.
      for (const provider of providers) {
        if (!current(job)) break;
        const key = catalogKey(provider);
        if (!present(key, job)) continue;
        const old = entriesRef.current[key];
        const retained =
          old &&
          current(old) &&
          (deadlines.current.get(old) ?? 0) > performance.now()
            ? old
            : null;
        put(
          key,
          {
            ...job,
            data: retained?.data,
            authority: retained?.authority,
            loading: true,
          },
          retained ? deadlines.current.get(retained) : undefined,
        );
        const query = {
          storeId: source.storeId,
          providerId: provider.id,
          expectedRevision: String(provider.revision),
        };
        try {
          await providersApi.writeProtocol(source.storeId);
          if (!present(key, job)) continue;
          const state = await providerSourceApi.state(query);
          if (!present(key, job)) continue;
          const authority = catalogAuthority(state);
          const previous =
            old && current(old) && old.authority === authority
              ? old.data
              : undefined;
          put(key, { ...job, authority, data: previous, loading: true });
          let data: typeof previous,
            failure: unknown,
            failed = false;
          try {
            data = await providerSourceApi.read(
              query,
              state.sourceRevision ?? null,
            );
          } catch (reason) {
            failure = reason;
            failed = true;
          }
          if (!present(key, job)) continue;
          // Recheck even on directory failure before retaining a prior snapshot.
          const latest = await providerSourceApi.state(query);
          if (!present(key, job)) continue;
          if (catalogAuthority(latest) !== authority)
            throw Error("目录来源或授权已变化，旧结果没有交付；请重新读取");
          put(key, {
            ...job,
            authority,
            data: failed ? previous : data,
            loading: false,
            error: failed ? message(failure) : undefined,
          });
        } catch (reason) {
          if (present(key, job))
            put(key, { ...job, loading: false, error: message(reason) });
        }
      }
    } finally {
      if (running.current === job) {
        running.current = null;
        if (current(job)) setPending(null);
      }
    }
  };
  return {
    catalogs,
    reading: active && current(pending),
    available: !!storeId && active,
    readAll,
  };
}
