use local_agent_core::{
    Provider,
    billing::{self, AccountSnapshot},
    providers::subscription,
};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadError {
    Other(&'static str),
    SubscriptionKeyRequired,
    SubscriptionAuthorization,
    SubscriptionUnconfirmed,
}

impl From<&'static str> for ReadError {
    fn from(message: &'static str) -> Self {
        Self::Other(message)
    }
}

impl ReadError {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::Other(message) => message,
            Self::SubscriptionKeyRequired => "读取 M Plan 额度需要 MiniMax 控制台签发的订阅 Key。",
            Self::SubscriptionAuthorization => {
                "MiniMax 未授权读取 M Plan 额度，请在官方控制台核对订阅和订阅 Key。"
            }
            Self::SubscriptionUnconfirmed => {
                "MiniMax 暂未确认有效订阅或限制了额度查询频率，请在官方控制台核对后稍后刷新。"
            }
        }
    }

    pub(super) fn state(self) -> Option<&'static str> {
        match self {
            Self::Other(_) => None,
            Self::SubscriptionKeyRequired | Self::SubscriptionAuthorization => {
                Some("subscription_authorization_failed")
            }
            Self::SubscriptionUnconfirmed => Some("subscription_unconfirmed"),
        }
    }
}

pub fn adapter(provider: &Provider) -> Option<(&'static str, &'static str)> {
    if subscription::minimax_plan(provider) {
        subscription::validate_endpoint(provider).ok()?;
        return Some((
            subscription::MINIMAX_PLAN,
            "https://www.minimax.io/v1/token_plan/remains",
        ));
    }
    let url = url::Url::parse(&provider.endpoint).ok()?;
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path = url.path().trim_end_matches('/');
    match (url.host_str()?, path) {
        ("api.deepseek.com", "" | "/v1" | "/anthropic") => {
            Some(("deepseek", "https://api.deepseek.com/user/balance"))
        }
        ("openrouter.ai", "/api/v1" | "/api") => {
            Some(("openrouter", "https://openrouter.ai/api/v1/key"))
        }
        ("api.moonshot.cn", "/v1" | "/anthropic") => {
            Some(("kimi-cn", "https://api.moonshot.cn/v1/users/me/balance"))
        }
        ("api.moonshot.ai", "/v1" | "/anthropic") => {
            Some(("kimi-global", "https://api.moonshot.ai/v1/users/me/balance"))
        }
        _ => None,
    }
}
pub(super) async fn read(
    client: &reqwest::Client,
    adapter: &str,
    url: &str,
    secret: &str,
) -> Result<AccountSnapshot, ReadError> {
    // A request crossing a reset boundary keeps its earlier observation window.
    let observed = chrono::Utc::now();
    let value = fetch(client, adapter, url, secret).await?;
    decode(adapter, &value, observed)
}

pub(super) fn decode(
    adapter: &str,
    value: &Value,
    observed: chrono::DateTime<chrono::Utc>,
) -> Result<AccountSnapshot, ReadError> {
    if adapter == subscription::MINIMAX_PLAN {
        match value["base_resp"]["status_code"].as_i64() {
            Some(1002) => return Err("服务商限制查询频率，请稍后刷新".into()),
            Some(1004 | 2049 | 2057) => return Err(ReadError::SubscriptionAuthorization),
            Some(2062) => return Err(ReadError::SubscriptionUnconfirmed),
            _ => (),
        }
    }
    billing::api_at(adapter, value, observed).map_err(Into::into)
}
pub(super) fn client() -> Result<reqwest::Client, &'static str> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "账户连接不可用")
}
async fn fetch(
    client: &reqwest::Client,
    adapter: &str,
    url: &str,
    secret: &str,
) -> Result<Value, ReadError> {
    let mut response = client
        .get(url)
        .timeout(std::time::Duration::from_secs(8))
        .bearer_auth(secret)
        .send()
        .await
        .map_err(|_| "账户查询网络失败，请稍后重试")?;
    match response.status().as_u16() {
        401 | 403 if adapter == subscription::MINIMAX_PLAN => {
            return Err(ReadError::SubscriptionAuthorization);
        }
        401 | 403 => return Err("账户查询未获授权，请检查此服务商凭据和账单读取权限".into()),
        429 => return Err("服务商限制查询频率，请稍后刷新".into()),
        200 => (),
        _ => return Err("账户接口未成功返回；未跟随跳转或记录服务商错误正文".into()),
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "账户响应读取中断")? {
        if body.len() + chunk.len() > 1024 * 1024 {
            return Err("账户响应超出限制".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| "账户响应格式不支持".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    #[test]
    fn pinned_origins_and_paths_only() {
        let p = |url: &str| Provider {
            id: "custom-test".into(),
            name: "Test".into(),
            vendor: String::new(),
            endpoint: url.into(),
            protocol: "messages".into(),
            model: "x".into(),
            configured: true,
            credential_ref: None,
            credential_label: None,
            models: vec![],
            disabled_models: vec![],
            model_catalog: None,
            revision: 1,
            category: "custom".into(),
        };
        assert!(adapter(&p("https://api.deepseek.com/anthropic")).is_some());
        let mut plan = p("https://api.minimax.io/anthropic");
        assert!(
            adapter(&plan).is_none(),
            "an arbitrary API connection is not a plan"
        );
        plan.vendor = subscription::MINIMAX_PLAN.into();
        assert_eq!(
            adapter(&plan),
            Some((
                subscription::MINIMAX_PLAN,
                "https://www.minimax.io/v1/token_plan/remains"
            ))
        );
        plan.endpoint = "https://api.minimax.io.evil.invalid/anthropic".into();
        assert!(adapter(&plan).is_none());
        for url in [
            "http://api.deepseek.com",
            "https://api.deepseek.com.evil.test",
            "https://api.deepseek.com:443/user",
            "https://api.deepseek.com/not-billing",
            "https://openrouter.ai/api/v1?token=x",
        ] {
            assert!(adapter(&p(url)).is_none(), "{url}");
        }
    }
    #[tokio::test]
    async fn get_only_bearer_redirects_and_secret_safe_errors() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for status in ["200 OK", "401 Unauthorized", "307 Temporary Redirect"] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut b = [0; 8192];
                let n = stream.read(&mut b).unwrap();
                let req = String::from_utf8_lossy(&b[..n]).to_lowercase();
                assert!(req.starts_with("get /"));
                assert!(req.contains("authorization: bearer disposable-key"));
                let body = if status.starts_with("200") {
                    r#"{"data":{"usage_monthly":0,"usage":3,"limit_remaining":0}}"#
                } else {
                    "disposable-key"
                };
                write!(stream,"HTTP/1.1 {status}\r\nContent-Length: {}\r\nLocation: http://127.0.0.1:1/\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        let client = client().unwrap();
        let snapshot = read(&client, "openrouter", &url, "disposable-key")
            .await
            .unwrap();
        assert_eq!(snapshot.money[2].amount.as_deref(), Some("0"));
        for _ in 0..2 {
            assert!(
                !read(&client, "openrouter", &url, "disposable-key")
                    .await
                    .err()
                    .unwrap()
                    .message()
                    .contains("disposable-key")
            );
        }
        server.join().unwrap();
    }

    #[tokio::test]
    async fn minimax_quota_rejections_keep_credentials_and_ambiguous_states_separate() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let responses = [
            (
                "401 Unauthorized",
                None,
                ReadError::SubscriptionAuthorization,
            ),
            ("403 Forbidden", None, ReadError::SubscriptionAuthorization),
            (
                "200 OK",
                Some(1002),
                ReadError::Other("服务商限制查询频率，请稍后刷新"),
            ),
            ("200 OK", Some(1004), ReadError::SubscriptionAuthorization),
            ("200 OK", Some(2049), ReadError::SubscriptionAuthorization),
            ("200 OK", Some(2057), ReadError::SubscriptionAuthorization),
            ("200 OK", Some(2062), ReadError::SubscriptionUnconfirmed),
        ];
        let server = std::thread::spawn(move || {
            for (status, code, _) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 8192];
                let n = stream.read(&mut request).unwrap();
                assert!(String::from_utf8_lossy(&request[..n]).contains("Bearer sk-cp-disposable"));
                let body = serde_json::json!({"base_resp":{"status_code":code,"status_msg":"sk-cp-disposable"}}).to_string();
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let client = client().unwrap();
        for (_, _, expected) in responses {
            let error = read(
                &client,
                subscription::MINIMAX_PLAN,
                &url,
                "sk-cp-disposable",
            )
            .await
            .unwrap_err();
            assert_eq!(error, expected);
            assert!(!error.message().contains("sk-cp-disposable"));
        }
        server.join().unwrap();
    }
}
