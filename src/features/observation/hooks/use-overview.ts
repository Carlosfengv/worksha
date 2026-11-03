import { observationApi } from "@/features/observation/api";
import type { Overview, OverviewQuery } from "@/features/observation/types";
import { useEffect, useState } from "react";
import {
  backendReadEpoch,
  subscribeBackendRecovery,
} from "@/shared/lib/backend-recovery";

function sameQuery(a: OverviewQuery, b: OverviewQuery) {
  return (
    (a.agent ?? undefined) === (b.agent ?? undefined) &&
    a.range === b.range &&
    a.metric === b.metric &&
    a.dimension === b.dimension &&
    a.source === b.source
  );
}

export function useOverview(query: OverviewQuery) {
  const { range, metric, dimension, source } = query;
  const agent = query.agent ?? undefined;
  const key = `${agent}:${range}:${metric}:${dimension}:${source}`;
  const [state, setState] = useState<{
    key: string;
    agent: string | undefined;
    data: Overview | null;
    error: string | null;
    epoch: symbol;
  }>({ key, agent, data: null, error: null, epoch: backendReadEpoch() });
  useEffect(() => {
    let alive = true;
    let pending: Promise<void> | null = null;
    let version = 0;
    const load = (): Promise<void> => {
      if (pending) return pending;
      const started = version;
      const epoch = backendReadEpoch();
      const request: Promise<void> = (async () => {
        try {
          const data = await observationApi.overview({
            agent,
            range,
            metric,
            dimension,
            source,
          });
          if (alive && started === version && epoch === backendReadEpoch())
            setState((old) => ({
              key,
              agent,
              epoch,
              data: {
                ...data,
                analysis:
                  old.data &&
                  old.epoch === epoch &&
                  sameQuery(old.data.analysis.query, {
                    agent,
                    range,
                    metric,
                    dimension,
                    source,
                  }) &&
                  Date.now() - Date.parse(old.data.analysis.asOf) < 10000
                    ? old.data.analysis
                    : data.analysis,
              },
              error: null,
            }));
        } catch (reason) {
          if (alive && started === version && epoch === backendReadEpoch())
            setState((old) => ({
              key,
              agent,
              epoch: old.agent === agent && old.data ? old.epoch : epoch,
              data: old.agent === agent ? old.data : null,
              error: reason instanceof Error ? reason.message : String(reason),
            }));
        }
      })().finally(() => {
        if (pending === request) pending = null;
      });
      pending = request;
      return request;
    };
    const unsubscribe = subscribeBackendRecovery(() => {
      version += 1;
      setState((old) => ({ ...old }));
      void (async () => {
        if (pending) await pending;
        if (alive) await load();
      })();
    });
    void load();
    const poll = () => {
      if (document.visibilityState !== "visible") return;
      // A slow read must not freeze old Token/s as a current measurement.
      if (pending)
        setState((old) =>
          old.data && Date.now() - Date.parse(old.data.asOf) > 6000
            ? { ...old }
            : old,
        );
      void load();
    };
    const resume = () => {
      if (document.visibilityState !== "visible") return;
      // Re-evaluate snapshot age before a slow resumed read completes.
      setState((old) => ({ ...old }));
      void load();
    };
    const timer = window.setInterval(poll, 1500);
    document.addEventListener("visibilitychange", resume);
    return () => {
      alive = false;
      unsubscribe();
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", resume);
    };
  }, [agent, range, metric, dimension, source, key]);
  return {
    // Analysis controls must not clear the independent activity cards while
    // their next statistics response is in flight. Agent scopes stay separate.
    data: state.agent === agent ? state.data : null,
    error: state.key === key ? state.error : null,
    updating:
      state.key !== key ||
      state.epoch !== backendReadEpoch() ||
      Boolean(state.data && !sameQuery(state.data.analysis.query, query)),
  };
}
