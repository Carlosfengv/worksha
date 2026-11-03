import fs from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";
const managed = new Set(
  JSON.parse(
    fs.readFileSync(".zeron/install-state.json", "utf8"),
  ).installations.flatMap((entry) => Object.keys(entry.fileHashes ?? {})),
);
const files = [];
function visit(directory) {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) visit(file);
    else if (/\.tsx?$/.test(file) && !managed.has(file)) files.push(file);
  }
}
visit("src");
visit("tests/frontend");
files.push(
  "playwright.config.ts",
  "scripts/check-architecture.mjs",
  "scripts/format.mjs",
);
const result = spawnSync(
  process.execPath,
  [
    "node_modules/prettier/bin/prettier.cjs",
    process.argv.includes("--write") ? "--write" : "--check",
    ...files,
  ],
  { stdio: "inherit" },
);
process.exit(result.status ?? 1);
