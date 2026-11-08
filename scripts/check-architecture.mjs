import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import ts from "typescript";

const managed = new Set(
  JSON.parse(
    fs.readFileSync(".zeron/install-state.json", "utf8"),
  ).installations.flatMap((entry) => Object.keys(entry.fileHashes ?? {})),
);
function files(directory, extension) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const file = path.join(directory, entry.name);
    return entry.isDirectory()
      ? files(file, extension)
      : extension.test(file)
        ? [file]
        : [];
  });
}
const commands = new Map();
const graph = new Map();
for (const file of files("src", /\.tsx?$/).filter(
  (file) => !managed.has(file),
)) {
  const source = fs.readFileSync(file, "utf8");
  const ast = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true);
  const edges = [];
  for (const statement of ast.statements) {
    if (
      !ts.isImportDeclaration(statement) ||
      !ts.isStringLiteral(statement.moduleSpecifier)
    )
      continue;
    const specifier = statement.moduleSpecifier.text;
    const target = specifier.startsWith("@/")
      ? "src/" + specifier.slice(2)
      : specifier.startsWith(".")
        ? path.normalize(path.join(path.dirname(file), specifier))
        : null;
    assert(
      !file.startsWith("src/shared/") ||
        !target ||
        !/^src\/(app|features)\//.test(target),
      `${file}: shared must not depend on ${specifier}`,
    );
    assert(
      !specifier.startsWith("@tauri-apps/") ||
        file === "src/shared/lib/desktop-command.ts",
      `${file}: native access belongs in desktop-command`,
    );
    assert(
      !target ||
        !/\/page$|\/overview-page$|\/requests-page$/.test(target) ||
        ["src/app/App.tsx", "src/app/builtin-plugins.tsx"].includes(file),
      `${file}: compose pages in app, import domain APIs/components instead`,
    );
    const typeOnly =
      statement.importClause?.isTypeOnly ||
      (statement.importClause?.namedBindings &&
        ts.isNamedImports(statement.importClause.namedBindings) &&
        !statement.importClause.name &&
        statement.importClause.namedBindings.elements.every(
          (element) => element.isTypeOnly,
        ));
    if (target && !typeOnly) {
      const resolved = [
        target + ".ts",
        target + ".tsx",
        target + "/index.ts",
      ].find(fs.existsSync);
      if (resolved && !managed.has(resolved)) edges.push(resolved);
    }
  }
  graph.set(file, edges);
  function visit(node) {
    if (
      ts.isCallExpression(node) &&
      node.expression.kind === ts.SyntaxKind.ImportKeyword &&
      ts.isStringLiteral(node.arguments[0])
    ) {
      const target = node.arguments[0].text;
      assert(
        !/\/page$|\/overview-page$|\/requests-page$/.test(target) ||
          ["src/app/App.tsx", "src/app/builtin-plugins.tsx"].includes(file),
        `${file}: lazy pages belong in the app composition`,
      );
      if (target.startsWith("@/")) {
        const resolved = [
          "src/" + target.slice(2) + ".tsx",
          "src/" + target.slice(2) + ".ts",
        ].find(fs.existsSync);
        if (resolved && !managed.has(resolved)) edges.push(resolved);
      }
    }
    if (
      ts.isCallExpression(node) &&
      node.expression.getText(ast) === "command"
    ) {
      assert(
        /^(src\/features\/[^/]+|src\/app)\/api\.ts$/.test(file),
        `${file}: commands belong in feature api.ts`,
      );
      const [name, args] = node.arguments;
      assert(
        name && ts.isStringLiteral(name),
        `${file}: command name must be literal`,
      );
      assert(!commands.has(name.text), `Duplicate IPC adapter: ${name.text}`);
      assert(
        !args || ts.isObjectLiteralExpression(args),
        `${name.text}: explicit IPC arguments required`,
      );
      commands.set(
        name.text,
        args
          ? args.properties.map((property) => property.name.getText(ast)).sort()
          : [],
      );
    }
    ts.forEachChild(node, visit);
  }
  visit(ast);
}
const visiting = new Set(),
  visited = new Set();
function visit(file, trail = []) {
  assert(
    !visiting.has(file),
    `Runtime import cycle: ${[...trail, file].join(" -> ")}`,
  );
  if (visited.has(file)) return;
  visiting.add(file);
  for (const target of graph.get(file) ?? []) visit(target, [...trail, file]);
  visiting.delete(file);
  visited.add(file);
}
for (const file of graph.keys()) visit(file);
const original = JSON.parse(fs.readFileSync("tests/ipc-contract.json", "utf8"));
const extensions = JSON.parse(
  fs.readFileSync("tests/plugin-ipc-contract.json", "utf8"),
);
const advancedExtensions = JSON.parse(
  fs.readFileSync("tests/plugin-advanced-ipc-contract.json", "utf8"),
);
assert(
  Object.keys(advancedExtensions).every(
    (name) =>
      !Object.hasOwn(original, name) && !Object.hasOwn(extensions, name),
  ),
  "Advanced IPC cannot replace existing IPC",
);
assert(
  Object.keys(extensions).every((name) => !Object.hasOwn(original, name)),
  "Plugin IPC cannot replace existing IPC",
);
const domainCommands = JSON.parse(
  fs.readFileSync("tests/domain-ipc-contract.json", "utf8"),
);
assert(
  Object.keys(domainCommands).every(
    (name) =>
      !Object.hasOwn(original, name) &&
      !Object.hasOwn(extensions, name) &&
      !Object.hasOwn(advancedExtensions, name),
  ),
  "Domain IPC cannot replace existing IPC",
);
const providerCatalogCommands = JSON.parse(
  fs.readFileSync("tests/provider-catalog-ipc-contract.json", "utf8"),
);
assert(
  Object.keys(providerCatalogCommands).every(
    (name) =>
      ![original, extensions, advancedExtensions, domainCommands].some(
        (fixture) => Object.hasOwn(fixture, name),
      ),
  ),
  "Provider catalog IPC cannot replace existing IPC",
);
const providerConnectionCommands = JSON.parse(
  fs.readFileSync("tests/provider-connection-ipc-contract.json", "utf8"),
);
assert(
  Object.keys(providerConnectionCommands).every(
    (name) =>
      ![
        original,
        extensions,
        advancedExtensions,
        domainCommands,
        providerCatalogCommands,
      ].some((fixture) => Object.hasOwn(fixture, name)),
  ),
  "Provider connection IPC cannot replace existing IPC",
);
const providerSourceCommands = JSON.parse(
  fs.readFileSync("tests/provider-source-ipc-contract.json", "utf8"),
);
assert(
  Object.keys(providerSourceCommands).every(
    (name) =>
      ![
        original,
        extensions,
        advancedExtensions,
        domainCommands,
        providerCatalogCommands,
        providerConnectionCommands,
      ].some((fixture) => Object.hasOwn(fixture, name)),
  ),
  "Provider source IPC cannot replace existing IPC",
);
const sourceOperationCommands = JSON.parse(
  fs.readFileSync("tests/provider-source-operation-ipc-contract.json", "utf8"),
);
assert(
  Object.keys(sourceOperationCommands).every(
    (name) =>
      ![
        original,
        extensions,
        advancedExtensions,
        domainCommands,
        providerCatalogCommands,
        providerConnectionCommands,
        providerSourceCommands,
      ].some((fixture) => Object.hasOwn(fixture, name)),
  ),
  "Source operation IPC cannot replace existing IPC",
);
const draftCatalogCommands = JSON.parse(
  fs.readFileSync("tests/provider-draft-catalog-ipc-contract.json", "utf8"),
);
assert(
  Object.keys(draftCatalogCommands).every(
    (name) =>
      ![
        original,
        extensions,
        advancedExtensions,
        domainCommands,
        providerCatalogCommands,
        providerConnectionCommands,
        providerSourceCommands,
        sourceOperationCommands,
      ].some((fixture) => Object.hasOwn(fixture, name)),
  ),
  "Draft catalog IPC cannot replace existing IPC",
);
const draftGrantCommands = JSON.parse(
  fs.readFileSync("tests/provider-draft-grant-ipc-contract.json", "utf8"),
);
assert(
  Object.keys(draftGrantCommands).every(
    (name) =>
      ![
        original,
        extensions,
        advancedExtensions,
        domainCommands,
        providerCatalogCommands,
        providerConnectionCommands,
        providerSourceCommands,
        sourceOperationCommands,
        draftCatalogCommands,
      ].some((fixture) => Object.hasOwn(fixture, name)),
  ),
  "Draft grant IPC cannot replace existing IPC",
);
const draftRecoveryCommands = JSON.parse(
  fs.readFileSync("tests/provider-draft-recovery-ipc-contract.json", "utf8"),
);
assert(
  Object.keys(draftRecoveryCommands).every(
    (name) =>
      ![
        original,
        extensions,
        advancedExtensions,
        domainCommands,
        providerCatalogCommands,
        providerConnectionCommands,
        providerSourceCommands,
        sourceOperationCommands,
        draftCatalogCommands,
        draftGrantCommands,
      ].some((fixture) => Object.hasOwn(fixture, name)),
  ),
  "Draft recovery IPC cannot replace existing IPC",
);
const presetCommands = JSON.parse(
  fs.readFileSync("tests/provider-presets-ipc-contract.json", "utf8"),
);
assert(
  Object.keys(presetCommands).every(
    (name) =>
      ![
        original,
        extensions,
        advancedExtensions,
        domainCommands,
        providerCatalogCommands,
        providerConnectionCommands,
        providerSourceCommands,
        sourceOperationCommands,
        draftCatalogCommands,
        draftGrantCommands,
        draftRecoveryCommands,
      ].some((fixture) => Object.hasOwn(fixture, name)),
  ),
  "Preset IPC cannot replace existing IPC",
);
const routingPolicyCommands = JSON.parse(
  fs.readFileSync("tests/routing-policy-ipc-contract.json", "utf8"),
);
const existingCommands = {
  ...original,
  ...extensions,
  ...advancedExtensions,
  ...domainCommands,
  ...providerCatalogCommands,
  ...providerConnectionCommands,
  ...providerSourceCommands,
  ...sourceOperationCommands,
  ...draftCatalogCommands,
  ...draftGrantCommands,
  ...draftRecoveryCommands,
  ...presetCommands,
};
assert(
  Object.keys(routingPolicyCommands).every(
    (name) => !Object.hasOwn(existingCommands, name),
  ),
  "Pure routing policy IPC cannot replace existing IPC",
);
const objectContributionCommands = JSON.parse(
  fs.readFileSync("tests/object-contribution-ipc-contract.json", "utf8"),
);
assert(
  Object.keys(objectContributionCommands).every(
    (name) =>
      !Object.hasOwn(existingCommands, name) &&
      !Object.hasOwn(routingPolicyCommands, name),
  ),
  "Object contribution IPC cannot replace existing IPC",
);
const requestExportCommands = JSON.parse(
  fs.readFileSync("tests/request-export-ipc-contract.json", "utf8"),
);
assert(
  Object.keys(requestExportCommands).every(
    (name) =>
      !Object.hasOwn(existingCommands, name) &&
      !Object.hasOwn(routingPolicyCommands, name) &&
      !Object.hasOwn(objectContributionCommands, name),
  ),
  "Unified export IPC cannot replace existing IPC",
);
const policyRecoveryCommands = JSON.parse(
  fs.readFileSync("tests/routing-policy-recovery-ipc-contract.json", "utf8"),
);
assert(
  Object.keys(policyRecoveryCommands).every(
    (name) =>
      !Object.hasOwn(existingCommands, name) &&
      !Object.hasOwn(routingPolicyCommands, name) &&
      !Object.hasOwn(objectContributionCommands, name) &&
      !Object.hasOwn(requestExportCommands, name),
  ),
  "Policy scope recovery IPC cannot replace existing IPC",
);
const workbenchCommands = JSON.parse(
  fs.readFileSync("tests/workbench-ipc-contract.json", "utf8"),
);
assert(
  Object.keys(workbenchCommands).every(
    (name) =>
      !Object.hasOwn(existingCommands, name) &&
      !Object.hasOwn(routingPolicyCommands, name) &&
      !Object.hasOwn(objectContributionCommands, name) &&
      !Object.hasOwn(requestExportCommands, name) &&
      !Object.hasOwn(policyRecoveryCommands, name),
  ),
  "Workbench IPC cannot replace existing IPC",
);
const iamCommands = JSON.parse(
  fs.readFileSync("tests/iam-ipc-contract.json", "utf8"),
);
const expected = Object.fromEntries(
  Object.entries({
    ...original,
    ...extensions,
    ...advancedExtensions,
    ...domainCommands,
    ...providerCatalogCommands,
    ...providerConnectionCommands,
    ...providerSourceCommands,
    ...sourceOperationCommands,
    ...draftCatalogCommands,
    ...draftGrantCommands,
    ...draftRecoveryCommands,
    ...presetCommands,
    ...routingPolicyCommands,
    ...objectContributionCommands,
    ...requestExportCommands,
    ...policyRecoveryCommands,
    ...workbenchCommands,
    ...iamCommands,
  }).sort(),
);
assert.deepEqual(
  Object.fromEntries([...commands].sort()),
  expected,
  "Frontend IPC names/arguments changed; review compatibility before updating the contract fixture",
);
const rust = new Map();
for (const file of files("src-tauri/src/commands", /\.rs$/)) {
  const source = fs.readFileSync(file, "utf8");
  for (const match of source.matchAll(
    /#\[tauri::command\]\s+pub\(crate\) async fn (\w+)\(([\s\S]*?)\) -> Result/g,
  )) {
    const [, name, signature] = match;
    const args = [...signature.matchAll(/\b(\w+): /g)]
      .map((match) => match[1])
      .filter((name) => !["app", "backend"].includes(name))
      .map((name) =>
        name.replace(/_([a-z])/g, (_, letter) => letter.toUpperCase()),
      )
      .sort();
    assert(!rust.has(name), `Duplicate Rust command ${name}`);
    rust.set(name, args);
  }
}
assert.deepEqual(
  Object.fromEntries([...rust].sort()),
  expected,
  "Rust command signature drift",
);
const entry = fs.readFileSync("src-tauri/src/lib.rs", "utf8");
const registration = entry.match(/tauri::generate_handler!\[([\s\S]*?)\]/)?.[1];
assert(registration, "Missing Tauri command registration");
const names = [...registration.matchAll(/commands::\w+::(\w+)/g)]
  .map((match) => match[1])
  .sort();
assert.deepEqual(names, Object.keys(expected), "Tauri registration drift");
console.log(
  `通过：${expected ? Object.keys(expected).length : 0} 个 IPC 名称 / 参数 / 注册一致；业务依赖无循环，shared 无反向依赖。`,
);
