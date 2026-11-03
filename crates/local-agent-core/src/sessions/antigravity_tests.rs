use super::{CallQuery, NativeSessions, SessionQuery, antigravity, antigravity_proto::Message};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

fn var(mut n: u64) -> Vec<u8> {
    let mut v = vec![];
    while n >= 128 {
        v.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    v.push(n as u8);
    v
}
fn num(field: u32, n: u64) -> Vec<u8> {
    [var(u64::from(field) << 3), var(n)].concat()
}
fn bytes(field: u32, b: &[u8]) -> Vec<u8> {
    [
        var((u64::from(field) << 3) | 2),
        var(b.len() as u64),
        b.to_vec(),
    ]
    .concat()
}
fn text(field: u32, s: &str) -> Vec<u8> {
    bytes(field, s.as_bytes())
}
fn time(field: u32, seconds: i64) -> Vec<u8> {
    bytes(field, &num(1, seconds as u64))
}
fn join(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}
pub(super) struct Fixture {
    _home: tempfile::TempDir,
    pub(super) home: PathBuf,
    path: PathBuf,
    db: Connection,
    pub(super) id: String,
    project: PathBuf,
}
impl Fixture {
    pub(super) fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(antigravity::root(&home)).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let path = antigravity::root(&home).join(format!("{id}.db"));
        let db = Connection::open(&path).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
            CREATE TABLE trajectory_meta(trajectory_id text PRIMARY KEY,cascade_id text,trajectory_type integer,source integer);
            CREATE TABLE steps(idx integer PRIMARY KEY,step_type integer,status integer,metadata blob,step_payload blob,step_format integer DEFAULT 0);
            CREATE TABLE trajectory_metadata_blob(id text PRIMARY KEY DEFAULT 'main',data blob);").unwrap();
        db.execute("INSERT INTO trajectory_meta VALUES(?1,?1,1,1)", [&id])
            .unwrap();
        let project = home.join("project");
        std::fs::create_dir(&project).unwrap();
        let uri = url::Url::from_file_path(&project).unwrap().to_string();
        let meta = join(&[bytes(1, &text(1, &uri)), time(2, 1_790_000_000)]);
        db.execute(
            "INSERT INTO trajectory_metadata_blob VALUES('main',?1)",
            [meta],
        )
        .unwrap();
        let f = Self {
            _home: temp,
            home,
            path,
            db,
            id,
            project,
        };
        f.step(
            0,
            14,
            3,
            1_790_000_000,
            bytes(19, &text(1, "Antigravity fixture task")),
        );
        let tool = join(&[
            text(1, "call-native"),
            text(2, "lookup"),
            text(
                3,
                "{\"query\":\"safe\",\"api_key\":\"hidden-fixture-secret\"}",
            ),
        ]);
        let response = join(&[
            text(1, "visible reply"),
            text(16, "hidden-thinking-sentinel"),
            bytes(7, &tool),
        ]);
        let usage = join(&[num(2, 100), num(3, 20), num(5, 40)]);
        let meta = f.metadata(1_790_000_001, 3, Some(bytes(9, &usage)));
        f.insert(1, 15, 3, &meta, &bytes(20, &response));
        let meta = f.metadata(1_790_000_002, 3, Some(bytes(4, &tool)));
        f.insert(
            2,
            38,
            3,
            &meta,
            &bytes(
                47,
                &join(&[
                    text(1, "fixture-mcp"),
                    bytes(2, &tool),
                    text(3, "tool-result-sentinel"),
                ]),
            ),
        );
        f
    }
    fn metadata(&self, seconds: i64, status: u64, extra: Option<Vec<u8>>) -> Vec<u8> {
        let mut v = join(&[
            time(1, seconds),
            time(32, seconds),
            bytes(24, &text(8, "fixture-gemini")),
        ]);
        if status == 3 {
            v.extend(time(8, seconds + 1));
        }
        if let Some(extra) = extra {
            v.extend(extra);
        }
        v
    }
    fn insert(&self, index: usize, kind: u64, status: u64, meta: &[u8], body: &[u8]) {
        let payload = join(&[num(1, kind), num(4, status), bytes(5, meta), body.to_vec()]);
        self.db.execute("INSERT OR REPLACE INTO steps(idx,step_type,status,metadata,step_payload) VALUES(?1,?2,?3,?4,?5)",params![index as i64,kind as i64,status as i64,meta,payload]).unwrap();
    }
    fn step(&self, index: usize, kind: u64, status: u64, seconds: i64, body: Vec<u8>) {
        self.insert(
            index,
            kind,
            status,
            &self.metadata(seconds, status, None),
            &body,
        );
    }
    fn sessions(&self) -> NativeSessions {
        NativeSessions::new(self.home.clone(), &["antigravity".into()])
    }
}
#[test]
fn authorized_wal_history_flows_into_sessions_calls_usage_mcp_and_memory() {
    let f = Fixture::new();
    let s = NativeSessions::new(f.home.clone(), &[]);
    s.refresh().unwrap();
    assert_eq!(s.query(SessionQuery::default()).unwrap().total, 0);
    assert!(s.memory("antigravity", None, &Default::default()).is_err());
    s.set_enabled("antigravity", true).unwrap();
    s.refresh().unwrap();
    let page = s
        .query(SessionQuery {
            agent: Some("antigravity".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.total, 1);
    let session = &page.items[0];
    assert_eq!(session.agent_product_id.as_deref(), Some("antigravity"));
    assert_eq!(session.usage.total_tokens, Some(120));
    assert_eq!(session.usage.cached_input_tokens, Some(40));
    assert_eq!(session.project_path.as_deref(), f.project.to_str());
    assert_eq!(session.model_calls, 1);
    assert_eq!(session.mcp_calls, 1);
    let calls = s
        .calls(CallQuery {
            agent: Some("antigravity".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(calls.total, 2);
    assert!(calls.items.iter().all(|c| c.response_id.is_none()));
    let conv = s.conversation(&session.id, None, None).unwrap();
    let serialized = serde_json::to_string(&conv).unwrap();
    assert!(serialized.contains("visible reply"));
    assert!(serialized.contains("tool-result-sentinel"));
    assert!(!serialized.contains("hidden-thinking-sentinel"));
    assert!(!serialized.contains("hidden-fixture-secret"));
    let mcp = conv.items.iter().find(|e| e.kind == "mcp").unwrap();
    assert_eq!(mcp.call_id.as_deref(), Some("call-native"));
    assert!(mcp.model_response_id.is_some());
    assert_eq!(mcp.server.as_deref(), Some("fixture-mcp"));
    std::fs::write(f.project.join("GEMINI.md"), "project-rules").unwrap();
    assert!(
        s.memory("antigravity", None, &Default::default())
            .unwrap()
            .files
            .iter()
            .any(|p| p.name == "GEMINI.md")
    );
    s.set_enabled("antigravity", false).unwrap();
    assert_eq!(s.query(SessionQuery::default()).unwrap().total, 0);
    assert!(s.conversation(&session.id, None, None).is_err());
    assert!(s.memory("antigravity", None, &Default::default()).is_err());
}
#[test]
fn pagination_rejects_in_place_wal_edits_and_supports_call_anchors() {
    let f = Fixture::new();
    for n in 3..60 {
        f.step(
            n,
            15,
            3,
            1_790_000_002 + n as i64,
            bytes(20, &text(1, "additional reply")),
        );
    }
    let s = f.sessions();
    s.refresh().unwrap();
    let id = format!("antigravity:{}", f.id);
    let first = s.conversation(&id, None, None).unwrap();
    assert_eq!(first.items.len(), 40);
    let cursor = first.next_cursor.unwrap();
    let second = s.conversation(&id, Some(&cursor), None).unwrap();
    assert_eq!(second.items.len(), 20);
    let anchor = second.items[8].id.clone();
    let anchored = s.conversation(&id, None, Some(&anchor)).unwrap();
    assert!(anchored.items.iter().any(|e| e.id == anchor));
    f.step(
        40,
        15,
        3,
        1_790_000_050,
        bytes(20, &text(1, "edited in place")),
    );
    assert_eq!(
        s.conversation(&id, Some(&cursor), None).unwrap_err().code,
        "stale_snapshot"
    );
}
#[test]
fn unsupported_rows_and_tool_errors_do_not_become_model_failures_or_zero_usage() {
    let f = Fixture::new();
    f.db.execute(
        "UPDATE steps SET status=7,step_payload=?1 WHERE idx=2",
        [join(&[
            num(1, 38),
            num(4, 7),
            bytes(5, &f.metadata(1_790_000_002, 7, None)),
            bytes(
                47,
                &join(&[
                    text(1, "fixture-mcp"),
                    bytes(2, &text(2, "lookup")),
                    text(3, "tool failed"),
                ]),
            ),
        ])],
    )
    .unwrap();
    f.step(
        3,
        15,
        7,
        1_790_000_003,
        join(&[bytes(20, &[]), bytes(31, &text(1, "explicit API error"))]),
    );
    f.step(4, 15, 3, 1_790_000_004, bytes(20, &text(1, "unsupported")));
    f.db.execute("UPDATE steps SET step_format=99 WHERE idx=4", [])
        .unwrap();
    let s = f.sessions();
    s.refresh().unwrap();
    let conv = s
        .conversation(&format!("antigravity:{}", f.id), None, None)
        .unwrap();
    assert!(conv.session.partial);
    assert_eq!(conv.session.model_error_count, 1);
    assert_eq!(conv.session.usage.total_tokens, Some(120));
    assert_eq!(conv.items.iter().filter(|e| e.failure.is_some()).count(), 1);
    assert!(
        conv.items
            .iter()
            .find(|e| e.status == "error" && e.kind == "model")
            .unwrap()
            .usage
            .is_none()
    );
}
#[test]
fn latest_explicit_activity_expires_and_revocation_stops_it() {
    let f = Fixture::new();
    let now = chrono::Utc::now();
    let seconds = now.timestamp();
    let tool = join(&[text(1, "active-call"), text(2, "read_file")]);
    f.insert(
        3,
        132,
        2,
        &f.metadata(seconds, 2, Some(bytes(4, &tool))),
        &bytes(140, &[]),
    );
    let s = f.sessions();
    s.refresh().unwrap();
    let (rows, limited) = antigravity::activities(&s, now).unwrap();
    assert!(!limited);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].phase, "tool_requested");
    assert_eq!(rows[0].tool_names, vec!["read_file"]);
    assert_eq!(
        rows[0].message_started_at,
        rows[0].user_input.as_ref().map(|input| input.at.clone())
    );
    assert_eq!(
        rows[0].user_input.as_ref().unwrap().text,
        "Antigravity fixture task"
    );
    assert_eq!(rows[0].turn_ttft_ms, None);
    let mut watch = super::live::NativeWatch::default();
    let active = watch.poll(&s);
    assert_eq!(active.len(), 1);
    assert!(!active[0].stale);
    assert!(
        antigravity::activities(&s, now + chrono::Duration::seconds(31))
            .unwrap()
            .0
            .is_empty()
    );
    f.step(4, 14, 3, seconds + 1, bytes(19, &text(1, "new turn")));
    assert!(antigravity::activities(&s, now).unwrap().0.is_empty());
    let retained = watch.poll(&s);
    assert_eq!(retained.len(), 1);
    assert!(retained[0].stale);
    assert_eq!(retained[0].event_at, active[0].event_at);
    assert_eq!(retained[0].phase, "tool_requested");
    s.set_enabled("antigravity", false).unwrap();
    std::fs::rename(
        antigravity::root(&f.home),
        f.home.join("revoked-conversations"),
    )
    .unwrap();
    assert!(antigravity::activities(&s, now).unwrap().0.is_empty());
    assert!(watch.poll(&s).is_empty());
}
#[test]
fn sqlite_links_and_malformed_protobuf_are_rejected() {
    let f = Fixture::new();
    let outside = f.home.join("outside.db");
    std::fs::hard_link(&f.path, &outside).unwrap();
    assert!(antigravity::open(&f.home, &f.path).is_err());
    std::fs::remove_file(outside).unwrap();
    #[cfg(unix)]
    {
        let link = antigravity::root(&f.home).join(format!("{}.db", uuid::Uuid::new_v4()));
        std::os::unix::fs::symlink(&f.path, &link).unwrap();
        assert!(antigravity::open(&f.home, &link).is_err());
    }
    for malformed in [&[0u8][..], &[10, 255][..], &[255; 12][..], &[9, 1][..]] {
        assert!(Message::parse(malformed).is_err());
    }
    let path: &Path = &f.path;
    assert!(antigravity::open(&f.home, path).is_ok());
}

#[test]
fn repeated_call_ids_stay_separate_and_only_successful_explicit_writes_are_recorded() {
    let f = Fixture::new();
    let target = f.project.join("file.txt");
    std::fs::write(&target, "fixture").unwrap();
    let input = serde_json::json!({"TargetFile":target}).to_string();
    let tool = join(&[
        text(1, "same-call"),
        text(2, "write_to_file"),
        text(3, &input),
    ]);
    for index in [3, 4] {
        f.insert(
            index,
            15,
            3,
            &f.metadata(1_790_000_003 + index as i64, 3, None),
            &bytes(20, &bytes(7, &tool)),
        );
    }
    f.insert(
        5,
        132,
        3,
        &f.metadata(1_790_000_008, 3, Some(bytes(4, &tool))),
        &bytes(140, &[]),
    );
    f.insert(
        6,
        132,
        7,
        &f.metadata(1_790_000_009, 7, Some(bytes(4, &tool))),
        &bytes(140, &[]),
    );
    let s = f.sessions();
    s.refresh().unwrap();
    let catalog = s.lock().unwrap();
    let file = catalog.files.get(&f.path).unwrap();
    assert_eq!(file.writes.len(), 1);
    assert_eq!(file.writes[0].call_id, "same-call");
    let tools: Vec<_> = file
        .calls
        .iter()
        .filter(|c| c.name == "write_to_file")
        .collect();
    assert_eq!(tools.len(), 2);
    assert!(tools.iter().all(|c| c.model_response_id.is_none()));
    assert_ne!(tools[0].id, tools[1].id);
}
