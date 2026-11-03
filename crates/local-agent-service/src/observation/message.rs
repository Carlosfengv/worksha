//! Stable message identity within an explicitly identified Session. Only the
//! keyed digest is persisted; prompts, tool arguments and results are not.
use local_agent_core::sessions::activity_input::preview;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) fn key(directory: &std::path::Path) -> Result<zeroize::Zeroizing<String>, String> {
    let path = directory.join("activity-message.key");
    let file = local_agent_core::providers::files::read_protected(&path).map_err(|e| e.message)?;
    if let Some(bytes) = file.bytes {
        let value = String::from_utf8(bytes).map_err(|_| "活动消息标识密钥不可读")?;
        if file.stamp.mode != 0o600
            || value.len() != 32
            || !value.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("活动消息标识密钥不可读".into());
        }
        Ok(zeroize::Zeroizing::new(value))
    } else {
        let value = uuid::Uuid::new_v4().simple().to_string();
        super::private_file(&path, value.as_bytes(), 0o600)?;
        Ok(zeroize::Zeroizing::new(value))
    }
}

struct HashWriter<'a>(&'a mut Sha256);
impl std::io::Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn identity(
    body: &Value,
    session: Option<&str>,
    product: Option<&str>,
    secret: &str,
) -> Option<String> {
    let session = session?;
    let mut hash = Sha256::new();
    hash.update(b"local-agent-activity-message-v1\0");
    hash.update(secret.as_bytes());
    for value in [session, product.unwrap_or("")] {
        serde_json::to_writer(HashWriter(&mut hash), value).ok()?;
    }
    let mut count = 0_u64;
    if let Some(messages) = body["messages"]
        .as_array()
        .or_else(|| body["input"].as_array())
    {
        for message in messages {
            if message["role"] != "user" {
                continue;
            }
            let content = &message["content"];
            if content
                .as_array()
                .is_some_and(|parts| parts.iter().any(|p| p["type"] == "tool_result"))
                || preview(content).is_none()
            {
                continue;
            }
            count += 1;
            hash.update(count.to_le_bytes());
            if let Some(text) = content.as_str() {
                serde_json::to_writer(HashWriter(&mut hash), text).ok()?;
            } else if let Some(parts) = content.as_array() {
                for part in parts {
                    match part["type"].as_str() {
                        Some("text" | "input_text") => {
                            serde_json::to_writer(HashWriter(&mut hash), &part["text"]).ok()?;
                        }
                        Some("image" | "image_url" | "input_image") => {
                            // Cache hints can change between tool continuations.
                            for (key, value) in part.as_object()? {
                                if key != "cache_control" {
                                    serde_json::to_writer(HashWriter(&mut hash), &(key, value))
                                        .ok()?;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    } else if body["input"].is_string() && preview(&body["input"]).is_some() {
        count = 1;
        serde_json::to_writer(HashWriter(&mut hash), &body["input"]).ok()?;
    }
    if count == 0 {
        return None;
    }
    hash.update(count.to_le_bytes());
    hash.update(secret.as_bytes());
    Some(format!("message:{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn tools_and_thinking_keep_identity_but_repeat_inputs_new_sessions_and_long_suffixes_do_not() {
        let first = json!({"messages":[{"role":"user","content":"check"}]});
        let mut continuation = first.clone();
        continuation["messages"].as_array_mut().unwrap().extend([
            json!({"role":"assistant","content":[{"type":"thinking","thinking":"private"},{"type":"tool_use","name":"Read","input":{"path":"private"}}]}),
            json!({"role":"user","content":[{"type":"tool_result","content":"private result"}]})
        ]);
        let key = |v: &Value, session| identity(v, session, Some("claude-code-cli"), "private-key");
        assert_eq!(
            key(&first, Some("claude:s")),
            key(&continuation, Some("claude:s"))
        );
        continuation["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"user","content":"check"}));
        assert_ne!(
            key(&first, Some("claude:s")),
            key(&continuation, Some("claude:s"))
        );
        assert_ne!(
            key(&first, Some("claude:s")),
            key(&first, Some("claude:other"))
        );
        assert!(key(&first, None).is_none());
        let a = json!({"messages":[{"role":"user","content":format!("{}a","x".repeat(400))}]});
        let b = json!({"messages":[{"role":"user","content":format!("{}b","x".repeat(400))}]});
        assert_ne!(key(&a, Some("claude:s")), key(&b, Some("claude:s")));
        let raw = key(&first, Some("claude:s")).unwrap();
        assert!(!raw.contains("check") && !raw.contains("private"));
    }
}
