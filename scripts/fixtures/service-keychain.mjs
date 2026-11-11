import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

export function fixtureClaudeCredential(settings, data) {
  const helper = settings.apiKeyHelper?.match(/^'([^']+)' --credential '([0-9a-f-]{36})'$/);
  assert(helper && path.dirname(helper[1]) === path.join(data, "provider-configs", "bin"),
    "Expected the generated credential helper inside the isolated fixture");
  const result = spawnSync(helper[1], ["--credential", helper[2]], { encoding: "utf8", timeout: 10000 });
  assert.equal(result.status, 0, "The generated helper could not read the fixture Keychain entry");
  const token = result.stdout.trim();
  assert(/^[0-9a-f]{32}$/.test(token), "The helper did not return a local gateway token");
  return token;
}

export function cleanFixtureKeychain(data) {
  if (existsSync(data)) {
    const relative = path.relative(realpathSync(tmpdir()), realpathSync(data));
    assert(relative && !path.isAbsolute(relative) && !relative.split(path.sep).includes(".."),
      "Keychain cleanup requires an isolated temporary data directory");
  }
  const refs = new Set();
  const database = path.join(data, "local-agent.sqlite3");
  if (existsSync(database)) {
    const db = new DatabaseSync(database, { readOnly: true });
    try {
      const owners = db.prepare("SELECT json_extract(value, '$.operation.credentialRef') AS ref FROM local_token_owners").all();
      for (const { ref } of owners) refs.add(ref);
      const protectionKeys = db.prepare(
        "SELECT value FROM local_token_values WHERE domain = 4 AND name = 'protection-key.json' UNION ALL " +
        "SELECT value FROM credential_recovery_values WHERE domain = 3 AND name = 'protection-key.json'",
      ).all();
      for (const { value } of protectionKeys) {
        const { reference, abandoned } = JSON.parse(Buffer.from(value).toString());
        refs.add(reference);
        for (const ref of abandoned) refs.add(ref);
      }
    } finally {
      db.close();
    }
  }
  const backupRef = path.join(data, "provider-configs", "backup-key-v2.json");
  if (existsSync(backupRef)) refs.add(JSON.parse(readFileSync(backupRef, "utf8")));
  assert([...refs].every(ref => typeof ref === "string" && /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(ref)),
    "The isolated fixture contains an invalid Keychain reference");
  const security = (command, ref) => spawnSync(
    "/usr/bin/security",
    [command, "-a", ref, "-s", "app.localagent.desktop.provider"],
    { encoding: "utf8", timeout: 5000 },
  );
  const missing = result => result.stderr?.includes("could not be found");
  let removed = 0;
  for (const ref of refs) {
    const deleted = security("delete-generic-password", ref);
    if (deleted.status === 0) removed++;
    else assert(missing(deleted), "Fixture Keychain cleanup failed");
    assert(missing(security("find-generic-password", ref)), "A fixture Keychain entry remains");
  }
  return { fixtureKeychainCleanup: true, tracked: refs.size, removed };
}
