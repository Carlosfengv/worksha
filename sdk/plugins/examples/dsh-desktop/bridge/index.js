import fs from "node:fs/promises";
import { constants } from "node:fs";
import { randomUUID } from "node:crypto";
import path from "node:path";
import os from "node:os";
import { normalizeSession } from "./normalize.js";
export const name = "worksha-readonly-bridge";
export const inject = ["sessionController"];
export function apply(ctx, config) {
  const directory = config?.outputDirectory ?? path.join(os.homedir(), ".local", "share", "local-agent", "dsh-desktop");
  if (!path.isAbsolute(directory)) throw Error("The DSH export directory must be absolute");
  const output = path.join(directory, "facts.jsonl");
  let closed = false, busy = false;
  const cancellation = new AbortController();
  async function publish() {
    if (busy || closed) return;
    busy = true;
    let stage = "prepare-export";
    try {
      await fs.mkdir(directory, { recursive: true, mode: 0o700 });
      const stat = await fs.lstat(directory);
      if (!stat.isDirectory() || stat.isSymbolicLink() || stat.uid !== process.getuid() || (stat.mode & 0o077)) throw Error("Invalid export directory");
      const signal = AbortSignal.any([cancellation.signal, AbortSignal.timeout(15000)]);
      stage = "list-sessions";
      const { items } = await ctx.sessionController.list({}, signal);
      if (!Array.isArray(items) || items.length > 256) throw Error("DSH session list exceeds supported bound");
      const observedAt = new Date().toISOString();
      const records = [], projects = new Set();
      let historyErrors = 0;
      stage = "read-session-history";
      for (const item of items) {
        signal.throwIfAborted();
        let page;
        try {
          page = await ctx.sessionController.events({ address: { kind: "session", sessionId: item.sessionId }, maxMessages: 40 }, signal);
        } catch {
          signal.throwIfAborted();
          historyErrors++;
          page = { records: [], hasMore: true };
        }
        for (const row of normalizeSession(item, page, observedAt)) {
          if (row.fact.kind === "project") {
            if (projects.has(row.fact.value.nativeId)) continue;
            projects.add(row.fact.value.nativeId);
          }
          records.push(row);
        }
      }
      const generation = randomUUID();
      const text = [JSON.stringify({ snapshot: generation, records: records.length }), ...records.map((record) => JSON.stringify(record)), JSON.stringify({ commit: generation })].join("\n") + "\n";
      if (records.length > 4096 || Buffer.byteLength(text) > 8 * 1024 * 1024) throw Error("DSH export exceeds source budget");
      if (closed) return;
      stage = "write-export";
      const existing = await fs.lstat(output).catch((error) => { if (error.code !== "ENOENT") throw error; return null; });
      if (existing && (!existing.isFile() || existing.isSymbolicLink() || existing.nlink !== 1 || existing.uid !== process.getuid())) throw Error("Invalid export file");
      // Keep the authorized inode stable. The reader checks before/after stamps
      // and retains its previous complete snapshot during an overlapping write.
      const handle = await fs.open(output, constants.O_RDWR | constants.O_NOFOLLOW | (existing ? 0 : constants.O_CREAT | constants.O_EXCL), 0o600);
      try {
        const opened = await handle.stat();
        if (!opened.isFile() || opened.nlink !== 1 || opened.uid !== process.getuid() || (opened.mode & 0o077) || (existing && (existing.dev !== opened.dev || existing.ino !== opened.ino))) throw Error("Export file identity changed");
        signal.throwIfAborted();
        await handle.writeFile(text, "utf8"); await handle.truncate(Buffer.byteLength(text)); await handle.sync();
      } finally { await handle.close(); }
      const status = path.join(directory, `status-${generation}.tmp`);
      await fs.writeFile(status, JSON.stringify({ version: "0.2.2-rc.1", observedAt, sessions: items.length, records: records.length, historyErrors, bytes: Buffer.byteLength(text), modelCalls: 0 }) + "\n", { mode: 0o600, flag: "wx" });
      await fs.rename(status, path.join(directory, "status.json"));
    } catch {
      if (!closed) console.warn("worksha-readonly-bridge: DSH export failed at", stage);
    } finally { busy = false; }
  }
  ctx.effect(() => {
    void publish();
    const timer = setInterval(() => void publish(), 5000);
    return () => { closed = true; cancellation.abort(); clearInterval(timer); };
  }, "Local Agent read-only export");
}
