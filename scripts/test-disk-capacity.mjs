import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import {
  appendFileSync,
  mkdirSync,
  mkdtempSync,
  copyFileSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import {
  diskSample,
  percentile,
  physicalVolume,
} from "./lib/capacity-evidence.mjs";

const option = (name, fallback) => {
  const index = process.argv.indexOf(name);
  return index < 0 ? fallback : process.argv[index + 1];
};
let binary = option("--binary");
const reportPath = path.resolve(
  option("--report", ".work/plugin-capacity.json"),
);
const scratch = path.resolve(option("--scratch", ".work/capacity-scratch"));
const repeats = Number(option("--repeats", "5"));
const files = Number(option("--files", "1000"));
const calls = Number(option("--calls", "500"));
assert(Number.isInteger(repeats) && repeats > 0 && repeats <= 5);
assert(Number.isInteger(files) && files > 0 && files <= 5000);
assert(Number.isInteger(calls) && calls > 0 && calls <= 1000);
mkdirSync(scratch, { recursive: true });
mkdirSync(path.dirname(reportPath), { recursive: true });
let ownedBinaryDirectory;
if (!binary) {
  // Compile once before measuring; all rounds run the same immutable binary.
  const output = execFileSync(
    "cargo",
    [
      "test",
      "--release",
      "--locked",
      "-p",
      "local-agent-core",
      "--no-run",
      "--message-format=json",
    ],
    { encoding: "utf8", timeout: 20 * 60 * 1000, maxBuffer: 32 * 1024 ** 2 },
  );
  const artifact = output
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line))
    .find(
      (item) =>
        item.reason === "compiler-artifact" &&
        item.target.name === "local_agent_core" &&
        item.profile.test &&
        item.executable,
    );
  assert(artifact, "Core capacity test binary was not built");
  ownedBinaryDirectory = mkdtempSync(path.join(scratch, "binary-"));
  binary = path.join(ownedBinaryDirectory, "core-capacity-tests");
  copyFileSync(artifact.executable, binary);
}
binary = path.resolve(binary);
const report = {
  schema: 2,
  complete: false,
  binary,
  sha256: createHash("sha256").update(readFileSync(binary)).digest("hex"),
  volume: physicalVolume(scratch),
  files: files * 2,
  calls: files * calls * 2,
  requestedRepeats: repeats,
  requiredFreeBytes: 16 * 1024 ** 3,
  samplingMs: 1000,
  diskScope:
    "Named fixture files plus volume free-space minimum; volume includes unrelated activity and unlinked SQLite spills. TMPDIR and SQLITE_TMPDIR use the physical workspace volume. OS caches are not flushed.",
  samples: [],
};
const save = () =>
  writeFileSync(reportPath, JSON.stringify(report, null, 2) + "\n");
save();
try {
  for (let round = 1; round <= repeats; round++) {
    const temporary = mkdtempSync(path.join(scratch, "round-"));
    const start = performance.now();
    // APFS may release unlinked SQLite files asynchronously after child exit.
    // Wait before the next round without lowering the measured disk budget.
    let before = diskSample(temporary);
    const reclaimDeadline = Date.now() + 120000;
    while (
      before.volumeAvailableBytes <= report.requiredFreeBytes &&
      Date.now() < reclaimDeadline
    ) {
      await new Promise((resolve) => setTimeout(resolve, 1000));
      before = diskSample(temporary);
    }
    let logicalPeak = 0,
      allocatedPeak = 0,
      minAvailable = before.volumeAvailableBytes;
    let sampleError, timer;
    const sample = () => {
      try {
        const disk = diskSample(temporary);
        logicalPeak = Math.max(logicalPeak, disk.logicalBytes);
        allocatedPeak = Math.max(allocatedPeak, disk.allocatedBytes);
        minAvailable = Math.min(minAvailable, disk.volumeAvailableBytes);
      } catch (error) {
        sampleError = error;
      }
    };
    try {
      assert(
        before.volumeAvailableBytes > report.requiredFreeBytes,
        "At least 16 GiB free disk is required before a capacity round (including unlinked SQLite temporary tables)",
      );
      const result = await new Promise((resolve, reject) => {
        const child = spawn(
          "/usr/bin/time",
          [
            "-l",
            binary,
            "sessions::scan_bench_tests::native_directory_first_scan_and_statistics_capacity",
            "--ignored",
            "--exact",
            "--nocapture",
          ],
          {
            detached: true,
            env: {
              ...process.env,
              TMPDIR: temporary,
              SQLITE_TMPDIR: temporary,
              LOCAL_AGENT_NATIVE_FILES: String(files),
              LOCAL_AGENT_NATIVE_CALLS: String(calls),
              LOCAL_AGENT_NATIVE_ROUNDS: "1",
              LOCAL_AGENT_NATIVE_STREAMING: "1",
              LOCAL_AGENT_NATIVE_AGGREGATE_CHANGES: "1",
            },
            stdio: ["ignore", "pipe", "pipe"],
          },
        );
        let stdout = "",
          stderr = "",
          timedOut = false;
        child.stdout.on("data", (b) => {
          stdout += b;
          appendFileSync(`${reportPath}.round-${round}.partial.log`, b);
        });
        child.stderr.on("data", (b) => {
          stderr += b;
        });
        const deadline = setTimeout(
          () => {
            timedOut = true;
            try {
              process.kill(-child.pid, "SIGKILL");
            } catch (error) {
              if (error.code !== "ESRCH") throw error;
            }
          },
          20 * 60 * 1000,
        );
        child.on("error", (error) => {
          clearTimeout(deadline);
          reject(error);
        });
        child.on("close", (code, signal) => {
          clearTimeout(deadline);
          resolve({ code, signal, timedOut, stdout, stderr });
        });
        timer = setInterval(sample, 1000);
      });
      sample();
      writeFileSync(
        `${reportPath}.round-${round}.log`,
        result.stdout + "\n" + result.stderr,
      );
      assert.equal(
        result.code,
        0,
        `Capacity round ${round} failed; see raw log (${result.signal}, timeout=${result.timedOut})`,
      );
      if (sampleError) throw sampleError;
      const metrics = (prefix) =>
        result.stdout
          .split("\n")
          .filter((line) => line.startsWith(prefix))
          .map((line) => JSON.parse(line.slice(prefix.length)));
      const cold = metrics("NATIVE_CAPACITY ")[0];
      const incremental = metrics("NATIVE_INCREMENTAL ")[0];
      const refresh = metrics("NATIVE_REFRESH ");
      assert.equal(cold.calls, files * calls * 2);
      assert.equal(incremental.changedFiles, 1);
      assert.equal(incremental.loadedProvider, 1);
      assert.equal(incremental.loadedModel, 1);
      assert.deepEqual(
        refresh.map((item) => item.phase),
        ["rewrite", "delete", "restore"],
      );
      assert(refresh.every((item) => item.allPageStatisticsMatch));
      const maximumResidentBytes = Number(
        result.stderr.match(/(\d+)\s+maximum resident set size/)?.[1],
      );
      assert(
        maximumResidentBytes > 0 && maximumResidentBytes <= 2048 * 1024 ** 2,
        "Existing 2 GiB RSS budget exceeded",
      );
      const entry = {
        round,
        elapsedSeconds: (performance.now() - start) / 1000,
        cold,
        incremental,
        refresh,
        maximumResidentBytes,
        namedLogicalBytesPeak: logicalPeak,
        namedAllocatedBytesPeak: allocatedPeak,
        volumeAvailableBytesBefore: before.volumeAvailableBytes,
        volumeAvailableBytesMinimum: minAvailable,
        volumeAvailableBytesAfter: diskSample(temporary).volumeAvailableBytes,
      };
      report.samples.push(entry);
      console.log(JSON.stringify(entry));
      save();
    } catch (error) {
      report.failedRound = {
        round,
        error: error.message,
        namedAllocatedBytesPeak: allocatedPeak,
        volumeAvailableBytesMinimum: minAvailable,
      };
      throw error;
    } finally {
      clearInterval(timer);
      rmSync(temporary, { recursive: true, force: true });
    }
  }
  report.summary = {};
  for (const key of [
    "scan",
    "fees",
    "providerLoad",
    "provider",
    "modelLoad",
    "model",
  ]) {
    const values = report.samples.map((s) => s.cold[key]);
    report.summary[key] = {
      median: percentile(values, 50),
      p95: percentile(values, 95),
      min: Math.min(...values),
      max: Math.max(...values),
    };
  }
  report.complete = true;
} catch (error) {
  report.error = error.message;
  throw error;
} finally {
  save();
  if (ownedBinaryDirectory)
    rmSync(ownedBinaryDirectory, { recursive: true, force: true });
}
