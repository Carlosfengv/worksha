import { randomUUID } from "node:crypto";
import { mkdtempSync, mkdirSync, realpathSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

// Synthetic SQLite/Protobuf data shared by service and isolated native-window checks.
export function createAntigravityFixture(directoryPath) {
  const directory = directoryPath
    ? realpathSync(directoryPath)
    : realpathSync(
        mkdtempSync(path.join(tmpdir(), "local-agent-antigravity-")),
      );
  const home = path.join(directory, "source-home");
  const root = path.join(home, ".gemini/antigravity/conversations");
  const project = path.join(home, "project");
  mkdirSync(root, { recursive: true });
  mkdirSync(project, { recursive: true });
  writeFileSync(path.join(project, "GEMINI.md"), "Fixture project rule");
  const id = randomUUID();
  const db = new DatabaseSync(path.join(root, `${id}.db`));
  db.exec(`PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
    CREATE TABLE trajectory_meta(trajectory_id TEXT PRIMARY KEY,cascade_id TEXT);
    CREATE TABLE trajectory_metadata_blob(id TEXT PRIMARY KEY,data BLOB);
    CREATE TABLE steps(idx INTEGER PRIMARY KEY,step_type INTEGER,status INTEGER,metadata BLOB,step_payload BLOB,step_format INTEGER DEFAULT 0);`);
  function varint(value) {
    let n = BigInt(value);
    const bytes = [];
    while (n >= 128n) {
      bytes.push(Number(n & 127n) | 128);
      n >>= 7n;
    }
    bytes.push(Number(n));
    return Buffer.from(bytes);
  }
  const number = (field, value) =>
    Buffer.concat([varint(field * 8), varint(value)]);
  const blob = (field, value) =>
    Buffer.concat([
      varint(field * 8 + 2),
      varint(Buffer.byteLength(value)),
      Buffer.from(value),
    ]);
  const text = blob;
  const timestamp = (field, ms) =>
    blob(field, number(1, Math.floor(ms / 1000)));
  const join = (...values) => Buffer.concat(values);
  const now = Date.now() - 5000;
  const meta = join(
    blob(1, text(1, new URL(`file://${project}`).toString())),
    timestamp(2, now),
  );
  db.prepare("INSERT INTO trajectory_meta VALUES(?,?)").run(randomUUID(), id);
  db.prepare("INSERT INTO trajectory_metadata_blob VALUES('main',?)").run(meta);
  const tool = join(
    text(1, "call-fixture"),
    text(2, "lookup"),
    text(3, '{"query":"fixture","api_key":"redact-this-secret"}'),
  );
  function step(index, kind, body, extra = Buffer.alloc(0)) {
    const metadata = join(
      timestamp(1, now + index * 1000),
      timestamp(8, now + (index + 1) * 1000),
      blob(24, text(8, "fixture-model")),
      extra,
    );
    const payload = join(
      number(1, kind),
      number(4, 3),
      blob(5, metadata),
      body,
    );
    db.prepare(
      "INSERT INTO steps(idx,step_type,status,metadata,step_payload) VALUES(?,?,?,?,?)",
    ).run(index, kind, 3, metadata, payload);
  }
  step(0, 14, blob(19, text(1, "Antigravity service fixture")));
  step(
    1,
    15,
    blob(
      20,
      join(
        text(1, "Fixture reply"),
        text(16, "private-thinking-sentinel"),
        blob(7, tool),
      ),
    ),
    blob(9, join(number(2, 100), number(3, 20))),
  );
  step(
    2,
    38,
    blob(
      47,
      join(
        text(1, "fixture-mcp"),
        blob(2, tool),
        text(3, "Fixture MCP output"),
      ),
    ),
  );
  writeFileSync(
    path.join(home, ".gemini/antigravity/mcp_config.json"),
    JSON.stringify({
      mcpServers: {
        "fixture-mcp": {
          command: "never-execute-fixture",
          disabled: false,
          env: { TOKEN: "never-expose-config" },
        },
      },
    }),
  );
  return { directory, home, project, id, db };
}
