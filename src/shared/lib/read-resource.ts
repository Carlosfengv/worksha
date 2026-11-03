import { backendReadEpoch, subscribeBackendRecovery } from "./backend-recovery";

export interface ReadSnapshot<T> {
  readonly data: T | null;
  readonly error: string | null;
  readonly loading: boolean;
}

export interface ReadResource<T> {
  getSnapshot: () => ReadSnapshot<T>;
  subscribe: (listener: () => void) => () => void;
  reload: () => Promise<void>;
  invalidate: () => Promise<void>;
}

interface Options {
  interval?: number | false;
  events?: readonly string[];
}

export interface ReadResourceFamily<P, T> {
  key: (parameters: P) => string;
  bind: (key: string) => ReadResource<T>;
}

// Match the JSON parameters sent over IPC, including omitted undefined fields.
// Sorting object keys must not reorder arrays or merge different filter scopes.
function parameterKey(parameters: unknown): string {
  const key = JSON.stringify(parameters, (_key, value: unknown) =>
    value && typeof value === "object" && !Array.isArray(value)
      ? Object.fromEntries(
          Object.entries(value).sort(([a], [b]) =>
            a < b ? -1 : a > b ? 1 : 0,
          ),
        )
      : value,
  );
  if (key === undefined) throw new Error("读取参数必须可序列化为 JSON");
  return key;
}

function entry<T>(
  loader: () => Promise<T>,
  empty: ReadSnapshot<T>,
  options: Options,
  idle: () => void,
): ReadResource<T> {
  let snapshot = empty;
  const subscribers = new Set<{ notify: () => void }>();
  let pending: Promise<void> | null = null;
  let revision = 0;
  let published = -1;
  let epoch = backendReadEpoch();
  let timer: number | undefined;
  let releaseEvents: (() => void) | null = null;

  const publish = (next: ReadSnapshot<T>) => {
    // A completed read is a new snapshot even if a loader reuses its payload
    // object. getSnapshot remains stable between these explicit publications.
    snapshot = next;
    for (const subscriber of [...subscribers]) {
      if (!subscribers.has(subscriber)) continue;
      try {
        subscriber.notify();
      } catch {
        console.error("local-agent: read-resource-observer");
      }
    }
  };
  const synchronizeEpoch = () => {
    const current = backendReadEpoch();
    if (epoch !== current) {
      epoch = current;
      revision += 1;
    }
  };
  const releaseIfIdle = () => {
    if (subscribers.size || pending) return;
    releaseEvents?.();
    releaseEvents = null;
    idle();
  };
  const load = (background = false): Promise<void> => {
    if (!subscribers.size) return Promise.resolve();
    synchronizeEpoch();
    if (!background && (snapshot.error !== null || !snapshot.loading))
      publish({ ...snapshot, error: null, loading: true });
    if (pending) return pending;
    let started = -1;
    let startedEpoch = epoch;
    const request: Promise<void> = Promise.resolve()
      .then(() => {
        if (!subscribers.size) return;
        started = revision;
        startedEpoch = epoch;
        return loader().then((data) => {
          if (
            subscribers.size &&
            started === revision &&
            startedEpoch === backendReadEpoch()
          ) {
            // Set this before notifying: an observer may unsubscribe/remount.
            published = started;
            publish({ data, error: null, loading: false });
          }
        });
      })
      .catch((reason: unknown) => {
        if (
          subscribers.size &&
          started === revision &&
          startedEpoch === backendReadEpoch()
        ) {
          published = started;
          publish({
            ...snapshot,
            error: reason instanceof Error ? reason.message : String(reason),
            loading: false,
          });
        }
      })
      .finally(() => {
        if (pending === request) pending = null;
        synchronizeEpoch();
        if (!subscribers.size) {
          releaseIfIdle();
        } else if (published < revision) {
          // Mutations and a last-subscriber remount can occur between result
          // publication and settlement. Drive the missing read in both cases.
          return load(true);
        }
      });
    pending = request;
    return request;
  };
  const invalidate = (): Promise<void> => {
    synchronizeEpoch();
    revision += 1;
    return load();
  };
  const poll = () => {
    if (document.visibilityState === "visible") void load(true);
  };
  return {
    getSnapshot: () => snapshot,
    reload: () => load(),
    invalidate,
    subscribe: (notify) => {
      const subscriber = { notify };
      subscribers.add(subscriber);
      if (subscribers.size === 1) {
        synchronizeEpoch();
        if (!releaseEvents) {
          const recover = () => void invalidate();
          const unsubscribe = subscribeBackendRecovery(recover);
          for (const event of options.events ?? [])
            window.addEventListener(event, recover);
          releaseEvents = () => {
            unsubscribe();
            for (const event of options.events ?? [])
              window.removeEventListener(event, recover);
          };
        }
        if (options.interval !== false) {
          timer = window.setInterval(poll, options.interval ?? 10000);
          document.addEventListener("visibilitychange", poll);
        }
        void load();
      }
      return () => {
        if (!subscribers.delete(subscriber) || subscribers.size) return;
        window.clearInterval(timer);
        timer = undefined;
        document.removeEventListener("visibilitychange", poll);
        // A later view owns a new read even when the departed view's request
        // is still settling. Its data and errors must not cross this boundary.
        revision += 1;
        snapshot = empty;
        published = -1;
        // Retain only an in-flight task and its invalidation listeners. No
        // result or polling survives the last subscriber.
        releaseIfIdle();
      };
    },
  };
}

export function createReadResourceFamily<P, T>(
  loader: (parameters: P) => Promise<T>,
  options: Options = {},
): ReadResourceFamily<P, T> {
  const entries = new Map<string, ReadResource<T>>();
  const empty: ReadSnapshot<T> = { data: null, error: null, loading: true };
  return {
    key: parameterKey,
    // Binding is passive: abandoned renders and unvisited parameter sets
    // cannot populate the cache. Entries exist only while subscribed/in flight.
    bind: (key) => ({
      getSnapshot: () => entries.get(key)?.getSnapshot() ?? empty,
      subscribe: (listener) => {
        let current = entries.get(key);
        if (!current) {
          const created = entry(
            () => loader(JSON.parse(key) as P),
            empty,
            options,
            () => {
              if (entries.get(key) === created) entries.delete(key);
            },
          );
          current = created;
          entries.set(key, current);
        }
        return current.subscribe(listener);
      },
      reload: () => entries.get(key)?.reload() ?? Promise.resolve(),
      invalidate: () => entries.get(key)?.invalidate() ?? Promise.resolve(),
    }),
  };
}

export function createReadResource<T>(
  loader: () => Promise<T>,
  options?: Options,
): ReadResource<T> {
  return createReadResourceFamily<null, T>(loader, options).bind("null");
}
