//! Rank outside the main Store lock; every send remains admitted by the original host.
use super::super::{Checked, Manager, Result, fail, run_sync};
use super::{runtime::Context, store::Binding};
use local_agent_core::{
    domain::routing_policy::{RankRequest, RankResult},
    plugin::policy::FailureMode,
};
use std::sync::Arc;
#[derive(Clone)]
pub(crate) struct Choice {
    pub order: Option<Vec<String>>,
    context: Option<Context>,
}
impl Manager {
    pub(crate) fn policy_choice_check(&self, choice: &Choice) -> Result<()> {
        if let Some(c) = &choice.context {
            self.policy_check(c)?;
        }
        Ok(())
    }
    pub(crate) async fn policy_choice_alive(self: &Arc<Self>, choice: &Choice) -> Result<()> {
        let manager = self.clone();
        let choice = choice.clone();
        run_sync(move || manager.policy_choice_check(&choice)).await
    }
    pub(crate) async fn policy_rank(
        self: &Arc<Self>,
        store: String,
        request: RankRequest,
    ) -> Result<Choice> {
        let manager = self.clone();
        let service = request.service_id.clone();
        let version = request.service_version_id.clone();
        let (binding, failure) = run_sync(move || {
            let marker = {
                let cache = manager
                    .policy_presence
                    .read()
                    .map_err(|_| fail("unavailable"))?;
                if cache.unavailable
                    || cache.unknown.contains(
                        &uuid::Uuid::parse_str(&service)
                            .checked("invalid_input")?
                            .to_string(),
                    )
                {
                    return Err(fail("invalid_record"));
                }
                cache.scopes.get(&(store.clone(), service.clone())).cloned()
            };
            let Some(marker) = marker else {
                return Ok((None, None));
            };
            if marker.pending.is_some() {
                return Err(fail("result_pending"));
            }
            let Some(head) = &marker.head else {
                // The marker can precede a failed SQL unbind. Require its
                // durable tombstone before selecting native ordering.
                manager
                    .policy
                    .lock()
                    .checked("unavailable")?
                    .consistent_control(&store, &service)?;
                return Ok((None, None));
            };
            let failure = head.failure_mode;
            let binding = (|| {
                let h = manager.policy.lock().checked("unavailable")?;
                h.consistent_control(&store, &service)?;
                h.binding(&store, &service, &version)?
                    .ok_or(fail("grant_revoked"))
            })();
            match binding {
                Ok(b) => {
                    let failure = b.status.failure_mode;
                    Ok((Some(b), Some(failure)))
                }
                Err(_)
                    if failure == FailureMode::Builtin
                        && marker.version.as_deref() == Some(version.as_str()) =>
                {
                    Ok((None, Some(failure)))
                }
                Err(e) => Err(e),
            }
        })
        .await?;
        let Some(binding): Option<Binding> = binding else {
            return Ok(Choice {
                order: None,
                context: None,
            });
        };
        let manager = self.clone();
        let context = run_sync(move || {
            manager.policy_context(&binding.status.package, Some(binding.clone()))
        })
        .await;
        let ranked: Result<(Context, RankResult)> = match context {
            Ok(c) => self
                .policy_run(c.clone(), Some(request))
                .await
                .and_then(|r| r.map(|r| (c, r)).ok_or(fail("invalid_record"))),
            Err(e) => Err(e),
        };
        match ranked {
            Ok((context, result)) => Ok(Choice {
                order: Some(result.order),
                context: Some(context),
            }),
            Err(_) if failure == Some(FailureMode::Builtin) => Ok(Choice {
                order: None,
                context: None,
            }),
            Err(e) => Err(e),
        }
    }
}
