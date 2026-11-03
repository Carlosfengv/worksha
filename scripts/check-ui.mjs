import { createHash } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";

const record = JSON.parse(readFileSync(".zeron/install-state.json", "utf8"));
const managed = new Map();
for (const installation of record.installations) {
  for (const [file, hashes] of Object.entries(installation.fileHashes ?? {})) {
    managed.set(file, hashes.installedHash);
  }
}
const errors = [];
for (const [file, expected] of managed) {
  const actual = createHash("sha256").update(readFileSync(file)).digest("hex");
  if (actual !== expected) errors.push(`${file}: 与官方安装记录不同`);
}
function visit(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) visit(file);
    else if (
      !managed.has(file) &&
      /\.(css|tsx?)$/.test(file) &&
      file !== "src/styles/globals.css"
    ) {
      const content = readFileSync(file, "utf8");
      if (/#[\da-f]{3,8}\b|\b(?:rgb|rgba|hsl|hsla)\(/i.test(content))
        errors.push(`${file}: 业务样式包含硬编码颜色`);
      if (/!important|\[data-slot[\s=]/.test(content))
        errors.push(`${file}: 业务样式覆盖了组件内部规则`);
      if (
        /\b(?:bg|text|border|ring|shadow)-(?:slate|gray|zinc|neutral|stone|red|blue|green|yellow|amber|white|black)(?:-\d+)?\b/.test(
          content,
        )
      )
        errors.push(`${file}: 业务样式未使用 Zeron 语义颜色`);
    }
  }
}
visit("src");
if (errors.length) {
  console.error(errors.join("\n"));
  process.exit(1);
}
console.log(
  `通过：${managed.size} 个受管理文件与官方安装记录一致；业务样式使用语义变量。`,
);
