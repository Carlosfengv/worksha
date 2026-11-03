use super::{canonical, identity};
use crate::sessions::sanitize_label;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteFile {
    pub path: String,
    pub operation: String,
    pub added: Option<usize>,
    pub removed: Option<usize>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileWrite {
    pub id: String,
    pub project_id: String,
    pub session_id: String,
    pub agent_id: String,
    pub agent_product_id: Option<String>,
    pub turn_id: String,
    pub call_id: String,
    pub tool: String,
    pub occurred_at: Option<String>,
    pub source_line: usize,
    pub evidence: String,
    pub files: Vec<WriteFile>,
}
fn file(
    cwd: &str,
    path: &str,
    operation: &str,
    added: Option<usize>,
    removed: Option<usize>,
) -> Option<WriteFile> {
    if path.is_empty() || path.contains('\0') {
        return None;
    }
    let path = Path::new(path);
    let path = canonical(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(cwd).join(path)
    })?;
    let root = identity(cwd)?;
    if !path.starts_with(&root.path) {
        return None;
    }
    Some(WriteFile {
        path: sanitize_label(&path.to_string_lossy(), 2048),
        operation: operation.into(),
        added,
        removed,
    })
}
// A file is identified only by structured write tools or an apply_patch header.
// Shell commands, natural-language claims and opaque wrapper code are never inferred as writes.
pub fn proposed(name: &str, input: &Value, cwd: &str) -> Vec<WriteFile> {
    let parsed = input
        .as_str()
        .and_then(|s| serde_json::from_str::<Value>(s).ok());
    let v = parsed.as_ref().unwrap_or(input);
    let name = name.rsplit('.').next().unwrap_or(name);
    match name {
        "Write" => v["file_path"]
            .as_str()
            .and_then(|p| file(cwd, p, "write", None, None))
            .into_iter()
            .collect(),
        "Edit" | "MultiEdit" => v["file_path"]
            .as_str()
            .and_then(|p| file(cwd, p, "edit", None, None))
            .into_iter()
            .collect(),
        "apply_patch" => {
            let text = v
                .as_str()
                .or_else(|| v["patch"].as_str())
                .or_else(|| v["input"].as_str())
                .unwrap_or("");
            let mut files = Vec::<WriteFile>::new();
            for line in text.lines() {
                let header = [
                    ("*** Add File: ", "add"),
                    ("*** Update File: ", "edit"),
                    ("*** Delete File: ", "delete"),
                    ("*** Move to: ", "rename"),
                ]
                .into_iter()
                .find_map(|(prefix, operation)| line.strip_prefix(prefix).map(|p| (p, operation)));
                if let Some((path, operation)) = header {
                    if let Some(f) = file(
                        cwd,
                        path,
                        operation,
                        if operation == "delete" { None } else { Some(0) },
                        if operation == "delete" { None } else { Some(0) },
                    ) {
                        files.push(f);
                    }
                } else if let Some(f) = files.last_mut() {
                    if line.starts_with('+') {
                        f.added = f.added.map(|n| n + 1);
                    }
                    if line.starts_with('-') {
                        f.removed = f.removed.map(|n| n + 1);
                    }
                }
            }
            files.truncate(1000);
            files
        }
        "文件修改" => v
            .as_array()
            .map(|changes| {
                changes
                    .iter()
                    .filter_map(|c| {
                        file(
                            cwd,
                            c["path"].as_str()?,
                            c["kind"]
                                .as_str()
                                .or_else(|| c["kind"]["type"].as_str())
                                .unwrap_or("change"),
                            None,
                            None,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => vec![],
    }
}
pub fn explicit_success(tool: &str, output: &Value, error: bool) -> bool {
    if error {
        return false;
    }
    let parsed = output
        .as_str()
        .and_then(|s| serde_json::from_str::<Value>(s).ok());
    let value = parsed.as_ref().unwrap_or(output);
    if value["exit_code"].as_i64().is_some_and(|n| n != 0)
        || value["metadata"]["exit_code"]
            .as_i64()
            .is_some_and(|n| n != 0)
        || value["isError"].as_bool() == Some(true)
        || value
            .get("error")
            .is_some_and(|e| !e.is_null() && e != &Value::Bool(false))
    {
        return false;
    }
    let s = value
        .as_str()
        .or_else(|| value["output"].as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if [
        "error",
        "failed",
        "failure",
        "rejected",
        "cancelled",
        "denied",
        "permission",
    ]
    .iter()
    .any(|word| s.starts_with(word) || s.starts_with(&format!("{word}:")))
    {
        return false;
    }
    if tool.rsplit('.').next() == Some("apply_patch") {
        return s.starts_with("success") || value["success"].as_bool() == Some(true);
    }
    ["Write", "Edit", "MultiEdit"].contains(&tool.rsplit('.').next().unwrap_or(tool))
}
