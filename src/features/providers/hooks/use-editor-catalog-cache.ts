import {
  useEffect,
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type { DraftCatalogRequest } from "../../../../contracts/domain/provider-draft-catalog/types";
import {
  capabilityAccessEpoch,
  subscribeCapabilityAccess,
} from "@/shared/lib/capability-access";
import { providerDraftCatalogApi } from "../api";
import { message } from "../display";
import type { ProviderModelCatalog } from "../types";
import { useCatalogContext, type CatalogContext } from "./use-catalog-context";

type Admission = {
  context: CatalogContext;
  access: symbol;
  view: number;
  key: string;
  generation: number;
  request: DraftCatalogRequest;
};
type Entry = Admission & { data: ProviderModelCatalog };
const expired =
  "目录范围尚未重新核查，已撤下读取结果；已登记模型仍保留，请重新获取模型。";
export function useEditorCatalogCache(
  storeId: string | undefined,
  key: string,
  generation: number,
) {
  const context = useCatalogContext(storeId);
  const { isCurrent } = context;
  const access = useSyncExternalStore(
    subscribeCapabilityAccess,
    capabilityAccessEpoch,
  );
  const [entry, setEntry] = useState<Entry | null>(null);
  const [error, setError] = useState<string | null>(null);
  const slot = useRef(entry),
    view = useRef(0);
  const live = useRef({ key, generation });
  const deadlines = useRef(new WeakMap<Entry, number>());
  const current = useCallback(
    (job: Admission) =>
      isCurrent(job.context) &&
      job.access === capabilityAccessEpoch() &&
      job.view === view.current &&
      live.current.key === job.key &&
      live.current.generation === job.generation &&
      document.visibilityState !== "hidden",
    [isCurrent],
  );
  const publish = useCallback((value: Entry | null) => {
    slot.current = value;
    setEntry(value);
  }, []);
  useLayoutEffect(() => {
    live.current = { key, generation };
    if (slot.current && !current(slot.current)) publish(null);
    setError(null);
  }, [key, generation, storeId, context.epoch, access, current, publish]);
  useEffect(() => {
    const visibility = () => {
      if (document.visibilityState === "hidden") {
        view.current++;
        publish(null);
      }
    };
    document.addEventListener("visibilitychange", visibility);
    return () => document.removeEventListener("visibilitychange", visibility);
  }, [publish]);
  useEffect(() => {
    if (!entry || !current(entry)) return;
    let stopped = false,
      checking = false,
      nextCheck = 0;
    const retire = (reason: unknown) => {
      if (!stopped && slot.current === entry) {
        publish(null);
        setError(message(reason));
      }
    };
    const tick = () => {
      if (stopped || slot.current !== entry || !current(entry)) return;
      const now = performance.now();
      if ((deadlines.current.get(entry) ?? 0) <= now) {
        retire(expired);
        return;
      }
      if (checking || now < nextCheck) return;
      checking = true;
      nextCheck = now + 1000;
      void providerDraftCatalogApi
        .check(entry.request)
        .then(() => {
          if (stopped || slot.current !== entry || !current(entry)) return;
          if ((deadlines.current.get(entry) ?? 0) <= performance.now()) {
            retire(expired);
            return;
          }
          deadlines.current.set(entry, performance.now() + 5000);
        })
        .catch(retire)
        .finally(() => {
          checking = false;
        });
    };
    const timer = window.setInterval(tick, 250);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [
    entry,
    storeId,
    context.epoch,
    access,
    key,
    generation,
    current,
    publish,
  ]);
  const get = () => {
    const value = slot.current;
    return value &&
      current(value) &&
      (deadlines.current.get(value) ?? 0) > performance.now()
      ? value.data
      : null;
  };
  const read = async (
    request: DraftCatalogRequest,
    readKey: string,
    sourceGeneration: number,
  ) => {
    const source = context.capture();
    if (!source) return false;
    const job: Admission = {
      context: source,
      access: capabilityAccessEpoch(),
      view: view.current,
      key: readKey,
      generation: sourceGeneration,
      request,
    };
    if (!current(job)) return false;
    setError(null);
    let data: ProviderModelCatalog | undefined,
      failure: unknown,
      failed = false;
    try {
      data = await providerDraftCatalogApi.read(request);
    } catch (reason) {
      failure = reason;
      failed = true;
    }
    if (!current(job)) return false;
    try {
      await providerDraftCatalogApi.check(request);
    } catch (reason) {
      if (current(job)) publish(null);
      throw reason;
    }
    if (!current(job)) return false;
    if (failed) throw failure;
    if (!data) return false;
    const value = { ...job, data };
    deadlines.current.set(value, performance.now() + 5000);
    publish(value);
    return true;
  };
  return {
    catalog:
      entry &&
      entry.key === key &&
      entry.generation === generation &&
      current(entry) &&
      (deadlines.current.get(entry) ?? 0) > performance.now()
        ? entry.data
        : null,
    get,
    read,
    error,
  };
}
