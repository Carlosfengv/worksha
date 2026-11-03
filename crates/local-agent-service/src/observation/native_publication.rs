//! A short-held immutable publication, separate from the sampler's I/O lock.
use local_agent_core::sessions::{ActivityAuthorization, live::NativeActivity};

#[derive(Clone, Default)]
pub(crate) struct NativePublication {
    pub rows: Vec<NativeActivity>,
    pub checked_at: Option<String>,
    pub error: Option<String>,
    pub revision: u64,
    pub authorizations: Vec<ActivityAuthorization>,
}
