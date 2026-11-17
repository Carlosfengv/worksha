import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

// Explicit macOS capacity test. All Agent histories and SQL stores are generated
// in temporary directories by the Rust fixture; no user sources are opened.
assert.equal(process.platform, "darwin", "Peak RSS measurement requires macOS");
const singleAgent = process.argv.includes("--single-agent");
const mixed = process.argv.includes("--mixed-observation");
assert.ok(!(singleAgent && mixed), "Select only one capacity fixture");
const files = Number(process.env.LOCAL_AGENT_NATIVE_FILES ?? 1000);
const calls = Number(
  process.env.LOCAL_AGENT_NATIVE_CALLS ??
    (mixed ? 950 : singleAgent ? 1000 : 500),
);
const gatewayRows = mixed
  ? Number(process.env.LOCAL_AGENT_MIXED_GATEWAY_ROWS ?? 50_000)
  : 0;
const sourceCount = mixed || singleAgent ? 1 : 2;
const repeats = Number(
  process.env.LOCAL_AGENT_NATIVE_CAPACITY_REPEATS ?? (mixed ? 3 : 1),
);
const limitMiB = Number(
  process.env.LOCAL_AGENT_NATIVE_MEMORY_LIMIT_MIB ?? 2048,
);
assert.ok(Number.isInteger(files) && files >= 1 && files <= 5000);
assert.ok(Number.isInteger(calls) && calls >= 1 && calls <= 1000);
assert.ok(
  !mixed ||
    (Number.isInteger(gatewayRows) &&
      gatewayRows >= 1 &&
      gatewayRows <= 1_000_000),
);
assert.ok(Number.isInteger(repeats) && repeats >= 1 && repeats <= 5);
assert.ok(Number.isFinite(limitMiB) && limitMiB > 0);
const temporary = mkdtempSync(path.join(tmpdir(), "native-capacity-run-"));
const run = (command, args, env = process.env, reportProgress = false) =>
  new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "",
      stderr = "";
    child.stdout.on("data", (b) => {
      stdout += b;
      if (reportProgress) process.stdout.write(b);
    });
    child.stderr.on("data", (b) => {
      stderr += b;
    });
    child.on("error", reject);
    child.on("exit", (code) =>
      code === 0
        ? resolve({ stdout, stderr })
        : reject(Error(`${command} exited ${code}\n${stdout}\n${stderr}`)),
    );
  });
try {
  console.log(
    `${mixed ? "Mixed observation" : singleAgent ? "Native usage metadata" : "Native directory"} capacity: ${files * sourceCount} batches / ${files * calls * sourceCount} native responses / ${gatewayRows} gateway requests; run without other builds.`,
  );
  const build = await run("cargo", [
    "test",
    "--release",
    "--locked",
    "-p",
    "local-agent-core",
    "--no-run",
    "--message-format=json",
  ]);
  const executable = build.stdout
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line))
    .find(
      (item) =>
        item.reason === "compiler-artifact" &&
        item.target.name === "local_agent_core" &&
        item.profile.test &&
        item.executable,
    )?.executable;
  assert.ok(executable, "Core test executable missing");
  const benchmark = path.join(temporary, "benchmark");
  copyFileSync(executable, benchmark);
  const sha256 = createHash("sha256")
    .update(readFileSync(benchmark))
    .digest("hex");
  const samples = [];
  for (let round = 0; round < repeats; round++) {
    console.log(`Capacity sample ${round + 1}/${repeats} started.`);
    const result = await run(
      "/usr/bin/time",
      [
        "-l",
        benchmark,
        mixed
          ? "observation::sql::tests::mixed_capacity_tests::mixed_observation_capacity"
          : singleAgent
            ? "sessions::scan_bench_tests::native_single_agent_usage_statistics_capacity"
            : "sessions::scan_bench_tests::native_directory_first_scan_and_statistics_capacity",
        "--ignored",
        "--exact",
        "--nocapture",
      ],
      {
        ...process.env,
        LOCAL_AGENT_NATIVE_FILES: String(files),
        LOCAL_AGENT_NATIVE_CALLS: String(calls),
        ...(mixed
          ? { LOCAL_AGENT_MIXED_GATEWAY_ROWS: String(gatewayRows) }
          : {}),
        LOCAL_AGENT_NATIVE_ROUNDS: "1",
        LOCAL_AGENT_NATIVE_STREAMING: "1",
        LOCAL_AGENT_NATIVE_AGGREGATE_CHANGES: "1",
      },
      true,
    );
    const metric = (prefix) =>
      JSON.parse(
        result.stdout
          .split("\n")
          .find((line) => line.startsWith(prefix))
          .slice(prefix.length),
      );
    const rss = Number(
      result.stderr.match(/(\d+)\s+maximum resident set size/)?.[1],
    );
    assert.ok(Number.isSafeInteger(rss) && rss > 0, "Peak RSS missing");
    const sample = mixed
      ? {
          statistics: metric("MIXED_OBSERVATION "),
          maximumResidentBytes: rss,
        }
      : singleAgent
        ? {
            statistics: metric("NATIVE_SINGLE_AGENT "),
            maximumResidentBytes: rss,
          }
        : {
            cold: metric("NATIVE_CAPACITY "),
            incremental: metric("NATIVE_INCREMENTAL "),
            refresh: result.stdout
              .split("\n")
              .filter((line) => line.startsWith("NATIVE_REFRESH "))
              .map((line) => JSON.parse(line.slice("NATIVE_REFRESH ".length))),
            maximumResidentBytes: rss,
          };
    if (mixed) {
      assert.equal(sample.statistics.nativeRows, files * calls);
      assert.equal(sample.statistics.gatewayRows, gatewayRows);
      assert.equal(sample.statistics.warmMs.length, 20);
      assert.equal(sample.statistics.updatedMs.length, 3);
      assert.ok(
        sample.statistics.rebuiltPositions.every((count) => count <= 1),
      );
      assert.ok(
        sample.statistics.updatedBlocks.every(
          ({ request, model }) => request <= 2 && model <= 2,
        ),
        "Fixture update rebuilt unaffected native blocks",
      );
    } else if (singleAgent) {
      assert.equal(sample.statistics.calls, files * calls);
      assert.equal(sample.statistics.changedFileRows, calls + 1);
      assert.equal(sample.statistics.allModelGroupsMatch, true);
    } else {
      assert.equal(sample.incremental.changedFiles, 1);
      assert.equal(sample.incremental.loadedProvider, 1);
      assert.equal(sample.incremental.loadedModel, 1);
      assert.deepEqual(
        sample.refresh.map((s) => s.phase),
        ["rewrite", "delete", "restore"],
      );
      assert.ok(sample.refresh.every((s) => s.allPageStatisticsMatch));
    }
    samples.push(sample);
    console.log(JSON.stringify(sample));
  }
  const sorted = (values) => [...values].sort((a, b) => a - b);
  const median = (values) => sorted(values)[Math.floor(values.length / 2)];
  const summary = mixed
    ? Object.fromEntries(
        ["requests", "overview", "modelUsage"].map((name, index) => {
          const metrics = samples.map((sample) => sample.statistics);
          const warm = sorted(
            metrics.flatMap((s) => s.warmMs.map((v) => v[index])),
          );
          return [
            name,
            {
              coldMedianMs: median(metrics.map((s) => s.coldMs[index])),
              legacyColdBudgetMs:
                median(metrics.map((s) => s.legacyGatewayMs[index])) +
                median(metrics.map((s) => s.legacyNativeMs[index])),
              warmP95Ms: warm[Math.ceil(warm.length * 0.95) - 1],
              incrementalMedianMs: Object.fromEntries(
                ["native", "late-gateway", "ambiguity"].map((phase, seq) => [
                  phase,
                  median(metrics.map((s) => s.updatedMs[seq][index])),
                ]),
              ),
            },
          ];
        }),
      )
    : undefined;
  const report = {
    schema: 1,
    profile: "release",
    case: mixed
      ? "mixed-observation"
      : singleAgent
        ? "single-agent-usage"
        : "mixed-native-directory",
    files: files * sourceCount,
    calls: files * calls * sourceCount,
    ...(mixed ? { gatewayRows, summary } : {}),
    executableSha256: sha256,
    memoryLimitMiB: limitMiB,
    samples,
  };
  if (process.env.LOCAL_AGENT_NATIVE_CAPACITY_REPORT)
    writeFileSync(
      process.env.LOCAL_AGENT_NATIVE_CAPACITY_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
  const rss = Math.max(...samples.map((sample) => sample.maximumResidentBytes));
  assert.ok(
    rss <= limitMiB * 1024 * 1024,
    `Peak RSS ${(rss / 1024 ** 2).toFixed(0)} MiB exceeds ${limitMiB} MiB`,
  );
  if (summary) {
    console.log(JSON.stringify(summary));
    for (const [label, metrics] of Object.entries(summary)) {
      const {
        coldMedianMs: cold,
        legacyColdBudgetMs: baseline,
        warmP95Ms: p95,
      } = metrics;
      assert.ok(
        cold <= baseline,
        `${label}: mixed cold ${cold}ms exceeds baseline ${baseline}ms`,
      );
      assert.ok(
        p95 < 5000,
        `${label}: warm P95 ${p95}ms exceeds polling interval`,
      );
      for (const [phase, incremental] of Object.entries(
        metrics.incrementalMedianMs,
      )) {
        assert.ok(
          incremental < cold,
          `${label}: ${phase} ${incremental}ms exceeds cold ${cold}ms`,
        );
      }
    }
  }
  console.log("Native capacity and incremental reconciliation passed.");
} finally {
  rmSync(temporary, { recursive: true, force: true });
}
