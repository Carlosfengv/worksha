//! One definition for installing and validating the existing raw-query hooks.
//! SQLite stores CREATE TRIGGER without IF NOT EXISTS or the trailing semicolon.
pub(in crate::observation) const TRIGGERS: &[(&str, &str)] = &[
    (
        "observation_query_insert",
        r#"CREATE TRIGGER observation_query_insert AFTER INSERT ON model_observations BEGIN
            UPDATE observation_query_clock SET seq=seq+1 WHERE id=1;
            INSERT INTO observation_query_changes VALUES(new.id,(SELECT seq FROM observation_query_clock)) ON CONFLICT(id) DO UPDATE SET seq=excluded.seq;
          END"#,
    ),
    (
        "observation_query_update",
        r#"CREATE TRIGGER observation_query_update AFTER UPDATE ON model_observations BEGIN
            UPDATE observation_query_clock SET seq=seq+1 WHERE id=1;
            INSERT INTO observation_query_changes VALUES(new.id,(SELECT seq FROM observation_query_clock)) ON CONFLICT(id) DO UPDATE SET seq=excluded.seq;
          END"#,
    ),
    (
        "observation_query_delete",
        r#"CREATE TRIGGER observation_query_delete AFTER DELETE ON model_observations BEGIN
            UPDATE observation_query_clock SET seq=seq+1 WHERE id=1;
            INSERT INTO observation_query_changes VALUES(old.id,(SELECT seq FROM observation_query_clock)) ON CONFLICT(id) DO UPDATE SET seq=excluded.seq;
          END"#,
    ),
    (
        "observation_query_move",
        r#"CREATE TRIGGER observation_query_move AFTER UPDATE OF id ON model_observations WHEN old.id<>new.id BEGIN
            UPDATE observation_query_clock SET seq=seq+1 WHERE id=1;
            INSERT INTO observation_query_changes VALUES(old.id,(SELECT seq FROM observation_query_clock)) ON CONFLICT(id) DO UPDATE SET seq=excluded.seq;
          END"#,
    ),
];
