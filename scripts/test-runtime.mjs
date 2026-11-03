import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { once } from "node:events";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import net from "node:net";
import http from "node:http";

const directory = realpathSync(
  mkdtempSync(path.join(tmpdir(), "local-agent-runtime-")),
);
const sourceHome = path.join(directory, "source-home");
mkdirSync(sourceHome, { mode: 0o700 });
const children = [];
const recordPath = path.join(directory, "runtime/service.json");
const readRecord = () => JSON.parse(readFileSync(recordPath, "utf8"));
const timeout = () =>
  new Promise((_, reject) =>
    setTimeout(() => reject(Error("Service timeout")), 10000).unref(),
  );
function start() {
  const child = spawn(
    "target/debug/local-agent-service",
    ["--data-dir", directory],
    {
      env: {
        ...process.env,
        LOCAL_AGENT_SERVICE_TOKEN: randomUUID(),
        LOCAL_AGENT_SOURCE_HOME: sourceHome,
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  children.push(child);
  let stderr = "";
  child.stderr.on("data", (bytes) => {
    stderr += bytes;
  });
  const lines = createInterface({ input: child.stdout });
  const ready = Promise.race([
    once(lines, "line").then(([line]) => ({ ready: JSON.parse(line) })),
    once(child, "exit").then(([code, signal]) => ({ code, signal, stderr })),
    timeout(),
  ]);
  return { child, ready };
}
async function request(record, route, options = {}) {
  return fetch(`http://127.0.0.1:${record.port}${route}`, {
    ...options,
    headers: { authorization: `Bearer ${record.token}`, ...options.headers },
    signal: AbortSignal.timeout(3000),
  });
}
async function stop(child, signal = "SIGTERM") {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, "exit");
  child.kill(signal);
  await Promise.race([exited, timeout()]);
}

try {
  const starts = [start(), start()];
  const outcomes = await Promise.all(starts.map((run) => run.ready));
  assert.equal(
    outcomes.filter((outcome) => outcome.ready).length,
    1,
    "one startup wins the service lock",
  );
  const winner = starts[outcomes.findIndex((outcome) => outcome.ready)];
  const record = readRecord();
  assert.equal(record.identity.pid, winner.child.pid);
  assert.equal(statSync(recordPath).mode & 0o777, 0o600);
  assert.equal(statSync(path.dirname(recordPath)).mode & 0o777, 0o700);
  const gatewayPath = path.join(directory, "gateway/listener.json");
  const gatewayBefore = readFileSync(gatewayPath, "utf8");
  const identity = await (await request(record, "/api/runtime")).json();
  assert.deepEqual(identity, record.identity);
  assert.ok(
    !JSON.stringify(identity).includes(record.token),
    "the token stays in the private connection record",
  );
  assert.equal((await request(record, "/api/snapshot")).status, 200);
  assert.equal(
    (
      await request(record, "/api/runtime", {
        headers: { authorization: "Bearer invalid" },
      })
    ).status,
    401,
  );
  assert.equal(
    (
      await request(record, "/api/runtime", {
        headers: { origin: "http://attacker.invalid" },
      })
    ).status,
    401,
  );
  const badHost = await new Promise((resolve, reject) => {
    const req = http.get(
      `http://127.0.0.1:${record.port}/api/runtime`,
      {
        headers: {
          Host: "attacker.invalid",
          Authorization: `Bearer ${record.token}`,
        },
      },
      (res) => {
        res.resume();
        resolve(res.statusCode);
      },
    );
    req.on("error", reject);
  });
  assert.equal(badHost, 401);
  assert.equal(
    (
      await request(record, "/api/runtime/shutdown", {
        method: "POST",
        headers: { authorization: "Bearer invalid" },
      })
    ).status,
    401,
  );

  const duplicate = start();
  assert.equal((await duplicate.ready).code, 1);
  assert.deepEqual(
    readRecord(),
    record,
    "a losing startup must not replace discovery or its token",
  );
  assert.equal(readFileSync(gatewayPath, "utf8"), gatewayBefore);
  assert.deepEqual(
    await (await request(readRecord(), "/api/runtime")).json(),
    identity,
    "reconnection keeps the same service process",
  );

  const exited = once(winner.child, "exit");
  assert.equal(
    (await request(record, "/api/runtime/shutdown", { method: "POST" })).status,
    200,
  );
  await Promise.race([exited, timeout()]);
  assert.equal(
    existsSync(recordPath),
    false,
    "normal shutdown removes its own discovery record",
  );

  const restart = start();
  assert.ok((await restart.ready).ready);
  const restarted = readRecord();
  assert.notEqual(restarted.identity.instanceId, record.identity.instanceId);
  assert.equal(
    readFileSync(gatewayPath, "utf8"),
    gatewayBefore,
    "the gateway port survives restart",
  );
  await stop(restart.child, "SIGKILL");
  assert.equal(
    existsSync(recordPath),
    true,
    "a crash may leave stale discovery",
  );
  const recovered = start();
  assert.ok((await recovered.ready).ready);
  assert.notEqual(
    readRecord().identity.instanceId,
    restarted.identity.instanceId,
  );
  assert.equal(readFileSync(gatewayPath, "utf8"), gatewayBefore);
  const recoveredExit = once(recovered.child, "exit");
  await request(readRecord(), "/api/runtime/shutdown", { method: "POST" });
  await Promise.race([recoveredExit, timeout()]);

  const port = JSON.parse(gatewayBefore);
  const blocker = net.createServer();
  blocker.listen(port, "127.0.0.1");
  await once(blocker, "listening");
  try {
    const blocked = start();
    const result = await blocked.ready;
    assert.equal(result.code, 1);
    assert.match(result.stderr, /固定端口被占用/);
    assert.equal(existsSync(recordPath), false);
    assert.equal(
      readFileSync(gatewayPath, "utf8"),
      gatewayBefore,
      "never replace a conflicting fixed gateway port",
    );
  } finally {
    blocker.close();
    await once(blocker, "close");
  }
  console.log(
    "通过：并发启动唯一服务、重复启动保留进程与端口、受保护发现、身份与鉴权、正常退出、崩溃遗留恢复、固定端口冲突保护。",
  );
} finally {
  await Promise.all(children.map((child) => stop(child)));
  chmodSync(directory, 0o700);
  rmSync(directory, { recursive: true, force: true });
}
