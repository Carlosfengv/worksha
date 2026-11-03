// Isolated real component transport; all history and keys are disposable fixtures.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { createHash, randomUUID } from "node:crypto";

const binary = path.resolve(
  process.env.PLUGIN_SOURCE_BINARY ?? "target/debug/local-agent-service",
);
const root = fs.mkdtempSync(
  path.join(fs.realpathSync(tmpdir()), "local-agent-source-worker-"),
);
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const report = {
  complete: false,
  binarySha256: sha(fs.readFileSync(binary)),
  cases: [],
  realModelCalls: 0,
  realKeychainCalls: 0,
};
let child;
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, "exit");
  child.kill("SIGKILL");
  await exited;
}
const sdk = (...args) =>
  execFileSync(binary, ["--plugin-sdk", ...args], {
    encoding: "utf8",
    timeout: 30000,
  });
try {
  const key = path.join(root, "key");
  sdk("keygen", key);
  const pkg = path.join(root, "history.laplug");
  sdk(
    "pack",
    path.resolve("sdk/plugins/examples/full-history/package"),
    pkg,
    key,
  );
  const checked = JSON.parse(sdk("check", pkg));
  assert.equal(checked.apiVersion, "1.0.0");
  assert.equal(checked.publisherTrusted, false);
  assert.equal(checked.manifest.manifestVersion, 1);
  report.cases.push(
    "signed history package uses the common API, package format and validator",
  );
  const manifest = checked.manifest;
  const knownUsage = {
    inputTokens: "10",
    outputTokens: "2",
    cachedInputTokens: null,
    cacheWriteInputTokens: null,
    reasoningOutputTokens: null,
    totalTokens: "12",
  };
  const rows = Array.from({ length: 255 }, (_, i) => ({
    fact: {
      kind: "project",
      value: { nativeId: `p${i}`, name: `项目 ${i}`, pathHint: null },
    },
  }));
  const events = Array.from({ length: 120 }, (_, i) => ({
    id: `event-${i}`,
    nativeId: `event-${i}`,
    turnId: `turn-${i}`,
    kind: "message",
    role: i % 2 ? "assistant" : "user",
    status: "recorded",
    text: `Fixture message ${i}`,
    input: null,
    output: null,
    model: null,
    responseId: null,
    usage: null,
  }));
  rows.push(
    {
      fact: {
        kind: "session",
        value: {
          nativeId: "s1",
          productIds: [`${manifest.id}:terminal`],
          unknownClient: false,
          title: "只读会话",
          createdAt: null,
          updatedAt: null,
          projectRef: "p0",
          usage: {
            ...knownUsage,
            inputTokens: "15",
            outputTokens: "4",
            totalTokens: "19",
          },
          partial: false,
        },
      },
      conversation: events,
    },
    {
      fact: {
        kind: "response",
        value: {
          nativeId: "r1",
          sessionRef: "s1",
          productId: `${manifest.id}:terminal`,
          occurredAt: null,
          model: "fixture",
          outcome: "success",
          usage: knownUsage,
          reportedResponseId: "unverified-id",
          durationMs: "10",
        },
      },
    },
    {
      fact: {
        kind: "activity",
        value: {
          nativeId: "a1",
          sessionRef: "s1",
          responseRef: "r1",
          occurredAt: null,
          phase: "generating",
          durationMs: "10",
        },
      },
    },
    {
      fact: {
        kind: "write",
        value: {
          nativeId: "w1",
          sessionRef: "s1",
          projectRef: "p0",
          responseRef: "r1",
          relativePath: "src/fixture.txt",
          occurredAt: null,
          outcome: "success",
        },
      },
    },
    {
      fact: {
        kind: "mcp-catalog",
        value: { nativeId: "m1", serverName: "fixture", toolNames: ["lookup"] },
      },
    },
    {
      fact: {
        kind: "mcp-execution",
        value: {
          nativeId: "e1",
          serverRef: "m1",
          sessionRef: "s1",
          responseRef: "r1",
          toolName: "lookup",
          occurredAt: null,
          durationMs: null,
          outcome: "unknown",
        },
      },
    },
  );
  const source = Buffer.from(
    rows.map((r) => JSON.stringify(r) + "\n").join(""),
  );
  const sourceRevision = sha(source);
  const contextId = randomUUID(),
    sourceRef = randomUUID();
  child = spawn(binary, ["--plugin-worker", pkg, checked.digest], {
    env: {},
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stderr = "";
  child.stderr.on("data", (b) => {
    stderr = (stderr + b).slice(-2000);
  });
  const lines = createInterface({ input: child.stdout })[
    Symbol.asyncIterator
  ]();
  let sequence = 0,
    reads = 0;
  async function nextLine() {
    let timer;
    try {
      const line = await Promise.race([
        lines.next(),
        new Promise((_, reject) => {
          timer = setTimeout(
            () => reject(Error(`worker frame timed out: ${stderr}`)),
            30000,
          );
        }),
      ]);
      assert(!line.done, `worker exited: ${stderr}`);
      return JSON.parse(line.value);
    } finally {
      clearTimeout(timer);
    }
  }
  const send = (v) => child.stdin.write(JSON.stringify(v) + "\n");
  async function exchange(method, params, unsupported = false) {
    const id = `host-${++sequence}`;
    send({ jsonrpc: "2.0", id, method, params });
    for (;;) {
      const v = await nextLine();
      assert.equal(v.jsonrpc, "2.0");
      if (v.method === "host.call") {
        assert.equal(v.params.contextId, contextId);
        assert.equal(v.params.method, "source.read");
        const request = JSON.parse(v.params.payload);
        assert.equal(request.contextId, contextId);
        assert.equal(request.sourceRef, sourceRef);
        const offset = Number(request.offset);
        assert(
          Number.isSafeInteger(offset) &&
            offset >= 0 &&
            offset <= source.length,
        );
        assert(request.maxBytes > 0 && request.maxBytes <= 262144);
        let end = Math.min(offset + request.maxBytes, source.length);
        while (
          end > offset &&
          end < source.length &&
          (source[end] & 0xc0) === 0x80
        )
          end--;
        reads++;
        send({
          jsonrpc: "2.0",
          id: v.id,
          result: JSON.stringify({
            text: source.subarray(offset, end).toString("utf8"),
            nextOffset: String(end),
            complete: end === source.length,
            sourceRevision,
          }),
        });
        continue;
      }
      assert.equal(v.id, id);
      if (unsupported) {
        assert.equal(v.error?.data?.code, "unsupported");
        return;
      }
      assert.equal(v.error, undefined, JSON.stringify(v.error));
      return v.result;
    }
  }
  await exchange("domain.scan", {}, true);
  const init = {
    apiVersion: "1.0.0",
    capabilities: manifest.capabilities,
    packageDigest: checked.digest,
    activationEpoch: "1",
    grantRevision: "1",
    contextId,
    fuel: "100000000",
    maxMemoryBytes: "134217728",
    settings: {},
  };
  assert.deepEqual(await exchange("plugin.initialize", init), init);
  for (const method of ["plugin.prepare", "plugin.activate", "plugin.health"])
    assert.deepEqual(await exchange(method, {}), { ready: true });
  assert.equal(reads, 0);
  await exchange("session.scan", {}, true);
  const invocation = {
    invocationId: contextId,
    providerKey: `${manifest.id}:domain.session-source@1:history`,
    implementationIdentity: checked.digest,
    epoch: "1",
    grantRevision: "1",
    catalogRevision: "1",
    resourceRef: sourceRef,
    deadlineMs: "30000",
    budget: {
      batchItems: 256,
      maxRecords: "4096",
      maxDocumentBytes: "10485760",
    },
  };
  const request = {
    invocation,
    sourceId: `${manifest.id}:history`,
    sourceRevision,
    cursor: null,
  };
  const first = await exchange("domain.scan", request);
  assert.equal(first.batch.records.length, 256);
  assert.equal(first.batch.batchId, "0");
  assert.equal(first.checkpoint, null);
  const second = await exchange("domain.scan", {
    ...request,
    cursor: first.nextCursor,
  });
  assert.equal(second.batch.batchId, "1");
  assert.equal(second.batch.records.length, rows.length - 256);
  assert.equal(second.checkpoint.records, String(rows.length));
  assert.equal(second.checkpoint.batches, "2");
  assert.equal(second.checkpoint.complete, true);
  assert.equal(second.nextCursor, null);
  const readCount = reads;
  const bodyRequest = { ...request, nativeId: "s1", anchor: null };
  const all = [];
  for (let cursor = null; ;) {
    const page = await exchange("domain.read-conversation", {
      ...bodyRequest,
      cursor,
    });
    assert.equal(page.sessionId, "s1");
    all.push(...page.events);
    cursor = page.nextCursor;
    if (cursor === null) {
      assert.equal(page.complete, true);
      break;
    }
  }
  assert.deepEqual(all, events);
  const anchor = await exchange("domain.read-conversation", {
    ...bodyRequest,
    anchor: "event-80",
  });
  assert.equal(anchor.events[0].nativeId, "event-80");
  assert.equal(reads, readCount);
  for (const method of ["plugin.deactivate", "plugin.dispose"])
    assert.deepEqual(await exchange(method, {}), { ready: true });
  const exited =
    child.exitCode === null ? await once(child, "exit") : [child.exitCode];
  assert.equal(exited[0], 0);
  report.cases.push(
    "source capability lifecycle rejects premature operations and legacy methods; preflight and metadata do not read resources",
  );
  report.cases.push(
    "real Wasm component returns two complete fact batches, all 120 body events and explicit anchor, with one pinned source load",
  );
  report.sourceReads = reads;
  report.factRecords = rows.length;
  report.bodyEvents = all.length;
  report.complete = true;
} finally {
  await stop();
  fs.rmSync(root, { recursive: true, force: true });
  report.ownedWorkerStopped =
    !child || child.exitCode !== null || child.signalCode !== null;
  report.temporaryDataRemoved = !fs.existsSync(root);
  if (process.env.PLUGIN_SOURCE_REPORT)
    fs.writeFileSync(
      process.env.PLUGIN_SOURCE_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
}
console.log(JSON.stringify(report, null, 2));
