import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = fileURLToPath(new URL("../", import.meta.url));
const release = process.argv.includes("--release");
const rustc = spawnSync("rustc", ["-vV"], { encoding: "utf8" });
if (rustc.status !== 0) throw new Error("Rust 工具链不可用");
const target =
  process.env.TAURI_ENV_TARGET_TRIPLE ??
  rustc.stdout.match(/^host: (.+)$/m)?.[1];
if (!target) throw new Error("无法识别 Rust 构建目标");
const args = ["build", "--locked", "-p", "local-agent-service"];
if (release) args.push("--release");
const host = rustc.stdout.match(/^host: (.+)$/m)?.[1];
if (target !== host) args.push("--target", target);
const build = spawnSync("cargo", args, { cwd: root, stdio: "inherit" });
if (build.status !== 0) process.exit(build.status ?? 1);
const suffix = target.includes("windows") ? ".exe" : "";
const profile = release ? "release" : "debug";
const source = path.join(
  root,
  "target",
  ...(target === host ? [] : [target]),
  profile,
  `local-agent-service${suffix}`,
);
const directory = path.join(root, "src-tauri", "binaries");
if (statSync(source).size > 128 * 1024 * 1024) {
  throw new Error(
    "本地服务超过凭据助手的 128 MiB 上限，未复制到应用包。请检查构建配置和产物体积。",
  );
}
mkdirSync(directory, { recursive: true });
copyFileSync(
  source,
  path.join(directory, `local-agent-service-${target}${suffix}`),
);
