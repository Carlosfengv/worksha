import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import {
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import http from "node:http";
import { createInterface } from "node:readline";
import { randomUUID } from "node:crypto";

const binary = path.resolve(
  process.env.PLUGIN_TEST_BINARY ?? "target/debug/local-agent-service",
);
const root = realpathSync(
  mkdtempSync(path.join(tmpdir(), "plugin-cancellation-")),
);
const report = { complete: false, binary, root, realModelCalls: 0 };
const data = path.join(root, "data"),
  home = path.join(root, "home");
mkdirSync(home);
const token = randomUUID();
let child, exited, base;
let entered;
const received = new Promise((resolve) => {
  entered = resolve;
});
let upstreamClosed = false;
const upstream = http.createServer((_request, response) => {
  response.on("close", () => {
    upstreamClosed = true;
  });
  entered(); // Intentionally do not return headers or body; this must cancel.
});
const sdk = (...args) =>
  execFileSync(binary, ["--plugin-sdk", ...args], { encoding: "utf8" });
async function api(route, body, method = body === undefined ? "GET" : "POST") {
  const response = await fetch(base + route, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(10000),
  });
  return { status: response.status, value: await response.json() };
}
try {
  await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
  const env = {
    ...process.env,
    LOCAL_AGENT_SOURCE_HOME: home,
    LOCAL_AGENT_SERVICE_TOKEN: token,
  };
  delete env.LOCAL_AGENT_TEST_MEMORY_VAULT;
  child = spawn(binary, ["--data-dir", data], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  exited = once(child, "exit");
  let stderr = "";
  child.stderr.on("data", (b) => {
    stderr += b;
  });
  const lines = createInterface({ input: child.stdout });
  const [line] = await Promise.race([
    once(lines, "line", { signal: AbortSignal.timeout(10000) }),
    exited.then(() => {
      throw Error(stderr);
    }),
  ]);
  base = `http://127.0.0.1:${JSON.parse(line).port}`;
  const snapshot = (await api("/api/snapshot")).value;
  const provider = {
    ...snapshot.providers.find((p) => p.id === "openai"),
    endpoint: `http://127.0.0.1:${upstream.address().port}/v1`,
    model: "fixture",
    models: ["fixture"],
    configured: true,
  };
  assert.equal((await api("/api/providers", provider, "PUT")).status, 200);
  const source = path.join(root, "package");
  cpSync("sdk/plugins/examples/pulse-quota/package", source, {
    recursive: true,
  });
  const key = path.join(root, "publisher.key");
  sdk("keygen", key);
  const archive = path.join(root, "quota.laplug");
  sdk("pack", source, archive, key);
  const review = (await api("/api/plugins/inspect", { path: archive })).value;
  const installed = await api("/api/plugins/install", {
    packageRef: review.packageRef,
    digest: review.digest,
    approvePublisher: true,
  });
  assert.equal(installed.status, 200);
  const query = {
    pluginId: installed.value.manifest.id,
    digest: installed.value.digest,
  };
  assert.equal(
    (await api("/api/plugins/state", { ...query, action: "enable" })).status,
    200,
  );
  assert.equal(
    (
      await api("/api/plugins/action", {
        ...query,
        action: {
          kind: "bind-connection",
          providerId: "openai",
          allowPrivateNetwork: true,
        },
      })
    ).status,
    200,
  );
  const pending = api("/api/plugins/action", {
    ...query,
    action: { kind: "refresh" },
  });
  let receiveTimer;
  try {
    await Promise.race([
      received,
      new Promise((_, reject) => {
        receiveTimer = setTimeout(
          () => reject(Error("Quota request not received")),
          10000,
        );
      }),
    ]);
  } finally {
    clearTimeout(receiveTimer);
  }
  assert.equal(
    (await api("/api/plugins/state", { ...query, action: "revoke-grants" }))
      .status,
    200,
  );
  const start = performance.now();
  const result = await pending;
  report.cancelAfterRevokeMs = performance.now() - start;
  report.resultCode = result.value.code;
  assert.equal(result.status, 403);
  assert.equal(result.value.code, "grant_revoked");
  assert(
    report.cancelAfterRevokeMs < 1500,
    "Revocation waited for the network timeout",
  );
  assert.equal((await api("/api/snapshot")).status, 200);
  for (let n = 0; n < 20 && !upstreamClosed; n++)
    await new Promise((r) => setTimeout(r, 25));
  assert(
    upstreamClosed,
    "Revoked quota request retained its upstream connection",
  );
  report.upstreamClosed = upstreamClosed;
  report.complete = true;
} catch (error) {
  report.error = error.message;
  throw error;
} finally {
  if (child?.exitCode === null) {
    try {
      await api("/api/runtime/shutdown", {});
      await exited;
    } finally {
      if (child.exitCode === null) {
        child.kill("SIGKILL");
        await exited;
      }
    }
  }
  upstream.closeAllConnections();
  await new Promise((r) => upstream.close(r));
  rmSync(path.join(root, "publisher.key"), { force: true });
  report.temporarySigningKeyRemoved = true;
  if (process.env.PLUGIN_CANCELLATION_REPORT)
    writeFileSync(
      process.env.PLUGIN_CANCELLATION_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
}
console.log(
  "Revocation cancels blocked quota networking and leaves the core available.",
);
