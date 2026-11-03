//! Local, dated test evidence. Never substitutes for a provider's advertised contract.
use super::{ProviderManager, files};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum Field {
    Tools,
    JsonOutput,
    JsonReliability,
    ContextMarker,
    ContextCapacity,
    MaxOutputTokens,
    StructuredOutput,
    Vision,
    Reasoning,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Passed,
    Failed,
    Unknown,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Check {
    pub field: Field,
    pub status: Status,
    pub detail: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Observation {
    pub model: String,
    pub client: String,
    pub client_version: String,
    pub checked_at: String,
    pub checks: Vec<Check>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    connection: String,
    observation: Observation,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u8,
    entries: Vec<Entry>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observations {
    pub state: &'static str,
    pub models: Vec<Observation>,
}

/// Exact connection identity; changing endpoint, protocol or credential invalidates the match.
pub fn connection_key(endpoint: &str, protocol: &str, credential_ref: Option<&str>) -> String {
    files::hash(&serde_json::to_vec(&(endpoint, protocol, credential_ref)).unwrap_or_default())
}
fn label(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
fn parse(bytes: &[u8], connection: &str, now: DateTime<Utc>) -> Option<Vec<Observation>> {
    let document: Document = serde_json::from_slice(bytes).ok()?;
    if document.version != 1 || document.entries.len() > 2000 {
        return None;
    }
    let mut result = vec![];
    for entry in document.entries {
        let o = entry.observation;
        let mut fields = BTreeSet::new();
        if entry.connection.len() != 64
            || !entry.connection.bytes().all(|c| c.is_ascii_hexdigit())
            || !label(&o.model, 200)
            || !label(&o.client, 80)
            || !label(&o.client_version, 64)
            || !DateTime::parse_from_rfc3339(&o.checked_at).is_ok_and(|time| time <= now)
            || o.checks.is_empty()
            || o.checks
                .iter()
                .any(|c| !label(&c.detail, 1200) || !fields.insert(c.field.clone()))
        {
            return None;
        }
        if entry.connection == connection {
            result.push(o);
        }
    }
    Some(result)
}

impl ProviderManager {
    pub fn model_observations(
        &self,
        endpoint: &str,
        protocol: &str,
        credential_ref: Option<&str>,
    ) -> Observations {
        let read = files::read(&self.directory.join("model-observations.json"));
        let models = match read {
            Ok(saved) => match saved.bytes {
                Some(bytes) => parse(
                    &bytes,
                    &connection_key(endpoint, protocol, credential_ref),
                    Utc::now(),
                ),
                None => Some(vec![]),
            },
            Err(_) => None,
        };
        Observations {
            state: if models.is_some() {
                "ready"
            } else {
                "unavailable"
            },
            models: models.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_evidence_is_connection_scoped_strict_and_preserves_unknown() {
        let key = connection_key("https://fixture.invalid", "messages", Some("credential-a"));
        let now = DateTime::parse_from_rfc3339("2026-10-26T06:00:00Z")
            .unwrap()
            .to_utc();
        let mut document = json!({"version":1,"entries":[{"connection":key,"observation":{
            "model":"model[1M]","client":"OpenCode","clientVersion":"1.18.13",
            "checkedAt":"2026-10-26T05:39:24Z","checks":[
                {"field":"tools","status":"passed","detail":"Read succeeded in the sample."},
                {"field":"jsonReliability","status":"unknown","detail":"One invalid output retained."}
            ]
        }}]});
        let encoded = serde_json::to_vec(&document).unwrap();
        let rows = parse(&encoded, &key, now).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            serde_json::to_value(&rows[0]).unwrap()["checks"][1]["status"],
            "unknown"
        );
        for other in [
            connection_key("https://other.invalid", "messages", Some("credential-a")),
            connection_key("https://fixture.invalid", "responses", Some("credential-a")),
            connection_key("https://fixture.invalid", "messages", Some("credential-b")),
        ] {
            assert!(parse(&encoded, &other, now).unwrap().is_empty());
        }
        document["entries"][0]["observation"]["checkedAt"] = json!("2026-10-27T00:00:00Z");
        assert!(parse(&serde_json::to_vec(&document).unwrap(), &key, now).is_none());
        document["entries"][0]["observation"]["checkedAt"] = json!("2026-10-26T00:00:00Z");
        document["entries"][0]["observation"]["checks"][0]["status"] = json!("supported");
        assert!(parse(&serde_json::to_vec(&document).unwrap(), &key, now).is_none());
        assert!(parse(b"not-json", &key, now).is_none());
    }
}
