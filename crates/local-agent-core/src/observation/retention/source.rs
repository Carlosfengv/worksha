//! Strict decoding is local to retention. Ordinary historical/remote readers
//! retain their compatibility policy; ignored source fields never authorize expiry.
use super::super::ModelObservation;

pub(super) fn observation(raw: &str) -> Result<ModelObservation, ()> {
    let mut ignored = false;
    let mut json = serde_json::Deserializer::from_str(raw);
    let row = serde_ignored::deserialize(&mut json, |_| ignored = true).map_err(|_| ())?;
    json.end().map_err(|_| ())?;
    if ignored { Err(()) } else { Ok(row) }
}
