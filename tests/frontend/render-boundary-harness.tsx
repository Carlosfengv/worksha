import { iamApi } from "@/features/iam/api";
import { Suspense, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import App from "@/app/App";
import { appApi } from "@/app/api";
import { builtinPluginsApi } from "@/features/plugins/api";
import {
  builtinBusinessPlugins,
  emptyBuiltinCatalog,
} from "./plugin-center-peers";
import {
  RenderBoundary,
  renderErrorOptions,
  retryableLazy,
} from "@/app/render-boundary";
import "@/styles/globals.css";

declare global {
  interface Window {
    recoveryFixture: {
      crash: boolean;
      loadFails: boolean;
      loads: number;
      writes: number;
      rootCrash: boolean;
    };
  }
}
window.recoveryFixture = {
  crash: true,
  loadFails: true,
  loads: 0,
  writes: 0,
  rootCrash: new URLSearchParams(location.search).has("root"),
};
iamApi.status = async () => ({
  enabled: false,
  available: true,
  account: null,
});
appApi.snapshot = async () => {
  throw new Error("fixture service offline");
};
builtinPluginsApi.catalog = async () => ({
  ...structuredClone(emptyBuiltinCatalog),
  host: { ...emptyBuiltinCatalog.host, plugins: builtinBusinessPlugins() },
});

function Content() {
  const [draft, setDraft] = useState("");
  if (window.recoveryFixture.crash)
    throw new Error("fixture-session-body-and-secret-do-not-log");
  return (
    <div>
      <label>
        草稿
        <input value={draft} onChange={(e) => setDraft(e.target.value)} />
      </label>
      <button onClick={() => window.recoveryFixture.writes++}>保存夹具</button>
      <p>页面已恢复</p>
    </div>
  );
}
const LazyContent = retryableLazy(async () => {
  window.recoveryFixture.loads++;
  if (window.recoveryFixture.loadFails)
    throw new Error("fixture-import-secret-do-not-log");
  return { default: Content };
});

function Harness() {
  const [route, setRoute] = useState(location.hash);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const changed = () => setRoute(location.hash);
    window.addEventListener("hashchange", changed);
    return () => window.removeEventListener("hashchange", changed);
  }, []);
  if (window.recoveryFixture.rootCrash)
    throw new Error("fixture-root-secret-do-not-log");
  return (
    <main>
      <nav aria-label="夹具导航">
        <a href="#/objects/a">对象 A</a>
        <a href="#/objects/b">对象 B</a>
        <button onClick={() => setTick((n) => n + 1)}>刷新夹具 {tick}</button>
      </nav>
      <RenderBoundary resetKey={route}>
        <Suspense fallback={<p role="status">正在加载夹具</p>}>
          {new URLSearchParams(location.search).has("lazy") ? (
            <LazyContent />
          ) : (
            <Content />
          )}
        </Suspense>
      </RenderBoundary>
    </main>
  );
}
createRoot(document.getElementById("root")!, renderErrorOptions).render(
  new URLSearchParams(location.search).has("app") ? (
    <App />
  ) : (
    <RenderBoundary scope="application" resetKey="application">
      <Harness />
    </RenderBoundary>
  ),
);
