//! Pure policy execution, authorization and durable publication intents are host-owned.
pub(super) mod api;
mod manage;
pub(super) mod package;
mod recovery;
mod runtime;
mod store;
pub(crate) use store::{Host, Presence};

mod dispatch;
pub(crate) use dispatch::{Choice, ChoiceProof};

#[cfg(test)]
mod tests;
