import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  createReadResourceFamily,
  type ReadResource,
} from "@/shared/lib/read-resource";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import { notifyBackendRecovery } from "@/shared/lib/backend-recovery";

type Value = { label: string };
type Parameters = {
  providerId: string;
  days: number;
  source?: string;
  ignored?: undefined;
};
type Read = {
  scope: string;
  parameters: Parameters;
  resolve: (value: Value) => void;
  reject: (error: Error) => void;
};
declare global {
  interface Window {
    sharedReads: {
      calls: Read[];
      throws: boolean;
      recover: () => void;
      saved?: () => Promise<void>;
      direct: ReadResource<Value>;
      stop?: () => void;
      replacement?: () => void;
      retained?: Value;
      bindingsAreEmpty: () => boolean;
    };
  }
}
const calls: Read[] = [];
const makeFamily = (scope: string) =>
  createReadResourceFamily(
    (parameters: Parameters) => {
      if (window.sharedReads.throws)
        throw new Error("synchronous shared loader failure");
      return new Promise<Value>((resolve, reject) =>
        calls.push({ scope, parameters, resolve, reject }),
      );
    },
    {
      events: ["fixture-read-changed"],
      interval: new URLSearchParams(location.search).has("oneshot")
        ? false
        : undefined,
    },
  );
const family = makeFamily("primary");
const other = makeFamily("other");
window.sharedReads = {
  calls,
  throws: false,
  recover: notifyBackendRecovery,
  direct: family.bind(family.key({ providerId: "direct", days: 7 })),
  bindingsAreEmpty: () =>
    Array.from({ length: 1000 }, (_, index) =>
      family.bind(family.key({ providerId: "unvisited-" + index, days: 7 })),
    ).every((resource) => resource.getSnapshot().data === null),
};
function Probe({
  id,
  source,
  providerId,
  enabled = true,
  alternate = false,
}: {
  id: string;
  source: string;
  providerId: string;
  enabled?: boolean;
  alternate?: boolean;
}) {
  const parameters =
    id === "a"
      ? { providerId, source, days: 7 }
      : { days: 7, ignored: undefined, source, providerId };
  const resource = useSharedQuery(
    alternate ? other : family,
    parameters,
    enabled,
  );
  return (
    <section aria-label={id}>
      <output data-testid={id + "-data"}>
        {resource.data?.label ?? "empty"}
      </output>
      <output data-testid={id + "-error"}>{resource.error}</output>
      <output data-testid={id + "-loading"}>{String(resource.loading)}</output>
      <button onClick={() => void resource.reload()}>reload-{id}</button>
      <button onClick={() => void resource.invalidate()}>
        invalidate-{id}
      </button>
      <button
        onClick={() => {
          window.sharedReads.saved = resource.invalidate;
        }}
      >
        save-{id}
      </button>
    </section>
  );
}
function Harness() {
  const [a, setA] = useState(true);
  const [b, setB] = useState(true);
  const [c, setC] = useState(false);
  const [enabled, setEnabled] = useState(true);
  const [source, setSource] = useState("gateway");
  const [providerId, setProviderId] = useState("provider");
  const direct = new URLSearchParams(location.search).has("direct");
  return (
    <>
      <button onClick={() => setA((v) => !v)}>toggle-a</button>
      <button onClick={() => setB((v) => !v)}>toggle-b</button>
      <button onClick={() => setC((v) => !v)}>toggle-c</button>
      <button onClick={() => setEnabled((v) => !v)}>enable-b</button>
      <button onClick={() => setSource("native")}>source-b</button>
      <button onClick={() => setProviderId("other-provider")}>
        provider-a
      </button>
      {!direct && a && (
        <Probe id="a" source="gateway" providerId={providerId} />
      )}
      {!direct && b && (
        <Probe id="b" source={source} providerId="provider" enabled={enabled} />
      )}
      {!direct && c && (
        <Probe id="c" source="gateway" providerId="provider" alternate />
      )}
    </>
  );
}
const root = document.getElementById("root");
if (!root) throw Error("Missing shared read fixture root");
createRoot(root).render(
  new URLSearchParams(location.search).has("strict") ? (
    <StrictMode>
      <Harness />
    </StrictMode>
  ) : (
    <Harness />
  ),
);
