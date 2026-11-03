import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
const binary = path.resolve(process.env.PLUGIN_TEST_BINARY ?? "target/debug/local-agent-service");
const run = (cmd, args) => { const result = spawnSync(cmd, args, { stdio: "inherit" }); if (result.status !== 0) throw new Error(`${cmd} failed`); };
run("cargo", ["build", "--manifest-path", "sdk/plugins/Cargo.toml", "--locked", "--workspace", "--release", "--target", "wasm32-unknown-unknown"]);
for (const [crate, target] of [
  ["aurora_agent_plugin", "sdk/plugins/examples/aurora-agent/package/plugin.wasm"],
  ["pulse_quota_plugin", "sdk/plugins/examples/pulse-quota/package/plugin.wasm"],
  ["envelope_protocol_plugin", "sdk/plugins/examples/envelope-protocol/package/plugin.wasm"],
  ["json_config_plugin", "sdk/plugins/examples/json-config/package/plugin.wasm"],
  ["full_history_plugin", "sdk/plugins/examples/full-history/package/plugin.wasm"],
  ["model_catalog_plugin", "sdk/plugins/examples/model-catalog/package/plugin.wasm"],
  ["pure_policy_plugin", "sdk/plugins/examples/pure-policy/package/plugin.wasm"],
  ["plugin_hostile_fixture", ".work/plugin-m4-hostile.wasm"],
]) {
  fs.mkdirSync(path.dirname(target), { recursive: true });
  run(binary, ["--plugin-sdk", "componentize", `sdk/plugins/target/wasm32-unknown-unknown/release/${crate}.wasm`, target]);
}
