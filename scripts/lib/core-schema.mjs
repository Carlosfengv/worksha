import { DatabaseSync } from "node:sqlite";

// Plugin integration tests compare the initialized core schema before/after
// plugin operations. Schema migration correctness belongs to core store tests.
export function readCoreSchema(file) {
  const db = new DatabaseSync(file, { readOnly: true });
  try {
    return {
      version: db.prepare("PRAGMA user_version").get().user_version,
      definitions: db
        .prepare(
          "SELECT type, name, tbl_name, sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )
        .all(),
    };
  } finally {
    db.close();
  }
}
