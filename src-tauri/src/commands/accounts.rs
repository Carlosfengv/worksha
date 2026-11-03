use crate::backend::Backend;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub(crate) async fn get_accounts(
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    backend
        .request(reqwest::Method::GET, "/api/accounts", None)
        .await
}
fn account_id(id: &str) -> Result<(), String> {
    if id.len() > 120
        || id.is_empty()
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("账户引用无效".into());
    }
    Ok(())
}
#[tauri::command]
pub(crate) async fn refresh_account(
    backend: tauri::State<'_, Backend>,
    account_ref: String,
    expected_revision: String,
) -> Result<serde_json::Value, String> {
    account_id(&account_ref)?;
    backend
        .request(
            reqwest::Method::POST,
            &format!("/api/accounts/{account_ref}/refresh"),
            Some(serde_json::json!({"expectedRevision":expected_revision})),
        )
        .await
}
#[tauri::command]
pub(crate) async fn create_claude_quota_launcher(
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/account-collectors/claude",
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn open_account_console(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
    account_ref: String,
) -> Result<(), String> {
    account_id(&account_ref)?;
    let data = backend
        .request(reqwest::Method::GET, "/api/accounts", None)
        .await?;
    let url = data["accounts"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == account_ref))
        .and_then(|row| row["officialUrl"].as_str())
        .ok_or("此账户没有已核对的官方入口")?;
    validate_console_url(url)?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|_| "官方页面未能打开".into())
}

fn validate_console_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "官方入口无效")?;
    if parsed.scheme() != "https"
        || (parsed.query().is_some() && url != "https://chatgpt.com/settings/usage?tab=overview")
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || ![
            "chatgpt.com",
            "claude.ai",
            "platform.openai.com",
            "platform.claude.com",
            "openrouter.ai",
            "platform.deepseek.com",
            "platform.kimi.com",
            "platform.kimi.ai",
        ]
        .contains(&parsed.host_str().unwrap_or(""))
    {
        return Err("官方入口不在允许范围".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{account_id, validate_console_url};

    #[test]
    fn account_references_cannot_change_the_service_path() {
        assert!(account_id("provider-123").is_ok());
        for id in ["", "../other", "a?x=1", "a/b", "账户", &"a".repeat(121)] {
            assert!(account_id(id).is_err());
        }
    }

    #[test]
    fn only_reviewed_https_consoles_and_the_exact_usage_query_are_opened() {
        assert!(validate_console_url("https://chatgpt.com/settings/usage?tab=overview").is_ok());
        assert!(validate_console_url("https://platform.deepseek.com/usage").is_ok());
        for url in [
            "http://claude.ai/",
            "file:///tmp/test",
            "https://claude.ai.evil.invalid/",
            "https://user:secret@claude.ai/",
            "https://claude.ai/?redirect=elsewhere",
            "https://chatgpt.com/settings/usage?tab=overview&other=1",
            "https://claude.ai/#fragment",
        ] {
            assert!(validate_console_url(url).is_err(), "{url}");
        }
    }
}
