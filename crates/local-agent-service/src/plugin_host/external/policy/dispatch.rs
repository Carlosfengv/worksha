//! Rank outside the main Store lock; every send remains admitted by the original host.
use super::super::{Checked, Manager, Result, fail, run_sync};
use super::{
    runtime::Context,
    store::{Authorization, Binding},
};
use local_agent_core::{
    domain::routing_policy::{RankRequest, RankResult},
    plugin::policy::FailureMode,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChoiceProof {
    control: Option<String>,
    binding_revision: Option<String>,
    authorization: Option<Authorization>,
}

#[derive(Clone)]
struct Scope {
    store: String,
    service: String,
    version: String,
}

#[derive(Clone)]
pub(crate) struct Choice {
    pub order: Option<Vec<String>>,
    context: Option<Context>,
    scope: Scope,
    pub proof: ChoiceProof,
}

impl Choice {
    fn new(
        scope: Scope,
        order: Option<Vec<String>>,
        context: Option<Context>,
        control: Option<String>,
    ) -> Self {
        let authorization = context.as_ref().and_then(|c| {
            c.binding
                .as_ref()
                .map(|b| Authorization::from_context(c, &b.instance))
        });
        let binding_revision = context
            .as_ref()
            .and_then(|c| c.binding.as_ref())
            .map(|b| b.status.revision.clone());
        Self {
            order,
            context,
            scope,
            proof: ChoiceProof {
                control,
                binding_revision,
                authorization,
            },
        }
    }
}

struct BindingDecision {
    binding: Option<Binding>,
    failure: Option<FailureMode>,
    control: Option<String>,
}

impl BindingDecision {
    fn allows_builtin(&self) -> bool {
        self.binding.is_none() || self.failure == Some(FailureMode::Builtin)
    }
}

impl Manager {
    pub(crate) fn policy_choice_check(&self, choice: &Choice) -> Result<()> {
        if let Some(context) = &choice.context {
            return self.policy_check(context);
        }
        // A permitted built-in fallback still belongs to the original
        // control revision, even when the plugin could not run.
        let current = self.policy_binding_decision(&choice.scope)?;
        if current.control != choice.proof.control || !current.allows_builtin() {
            return Err(fail("grant_revoked"));
        }
        Ok(())
    }
    pub(crate) async fn policy_choice_alive(self: &Arc<Self>, choice: &Choice) -> Result<()> {
        let manager = self.clone();
        let choice = choice.clone();
        run_sync(move || manager.policy_choice_check(&choice)).await
    }
    async fn policy_binding_for(self: &Arc<Self>, scope: &Scope) -> Result<BindingDecision> {
        let manager = self.clone();
        let scope = scope.clone();
        run_sync(move || manager.policy_binding_decision(&scope)).await
    }

    fn policy_binding_decision(&self, scope: &Scope) -> Result<BindingDecision> {
        let Scope {
            store,
            service,
            version,
        } = scope;
        let host = self.policy.lock().checked("unavailable")?;
        let marker = {
            let cache = self
                .policy_presence
                .read()
                .map_err(|_| fail("unavailable"))?;
            if cache.unavailable
                || cache.unknown.contains(
                    &uuid::Uuid::parse_str(service)
                        .checked("invalid_input")?
                        .to_string(),
                )
            {
                return Err(fail("invalid_record"));
            }
            cache.scopes.get(&(store.clone(), service.clone())).cloned()
        };
        let Some(marker) = marker else {
            return Ok(BindingDecision {
                binding: None,
                failure: None,
                control: None,
            });
        };
        if marker.pending.is_some() {
            return Err(fail("result_pending"));
        }
        let control = Some(format!("{}/{}", marker.authority, marker.revision));
        let Some(head) = &marker.head else {
            // The marker can precede a failed SQL unbind. Require its
            // durable tombstone before selecting native ordering.
            host.consistent_control(store, service)?;
            return Ok(BindingDecision {
                binding: None,
                failure: None,
                control,
            });
        };
        let failure = head.failure_mode;
        let binding = (|| {
            host.consistent_control(store, service)?;
            host.binding(store, service, version)?
                .ok_or(fail("grant_revoked"))
        })();
        match binding {
            Ok(b) => {
                let failure = b.status.failure_mode;
                Ok(BindingDecision {
                    binding: Some(b),
                    failure: Some(failure),
                    control,
                })
            }
            Err(_)
                if failure == FailureMode::Builtin
                    && marker.version.as_deref() == Some(version.as_str()) =>
            {
                Ok(BindingDecision {
                    binding: None,
                    failure: Some(failure),
                    control,
                })
            }
            Err(e) => Err(e),
        }
    }

    /// The explicitly configured classifier is a separate model call. A
    /// fail-closed policy must still have a live grant before that call.
    pub(crate) async fn policy_classifier_guard(
        self: &Arc<Self>,
        store: String,
        service: String,
        version: String,
    ) -> Result<Choice> {
        let scope = Scope {
            store,
            service,
            version,
        };
        let BindingDecision {
            binding,
            failure,
            control,
        } = self.policy_binding_for(&scope).await?;
        let Some(binding) = binding else {
            return Ok(Choice::new(scope, None, None, control));
        };
        let manager = self.clone();
        match run_sync(move || {
            manager.policy_context(&binding.status.package, Some(binding.clone()))
        })
        .await
        {
            Ok(context) => Ok(Choice::new(scope, None, Some(context), control)),
            Err(_) if failure == Some(FailureMode::Builtin) => {
                Ok(Choice::new(scope, None, None, control))
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn policy_rank(
        self: &Arc<Self>,
        store: String,
        request: RankRequest,
    ) -> Result<Choice> {
        let scope = Scope {
            store,
            service: request.service_id.clone(),
            version: request.service_version_id.clone(),
        };
        let BindingDecision {
            binding,
            failure,
            control,
        } = self.policy_binding_for(&scope).await?;
        let Some(binding) = binding else {
            return Ok(Choice::new(scope, None, None, control));
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
            Ok((context, result)) => Ok(Choice::new(
                scope,
                Some(result.order),
                Some(context),
                control,
            )),
            Err(_) if failure == Some(FailureMode::Builtin) => {
                Ok(Choice::new(scope, None, None, control))
            }
            Err(e) => Err(e),
        }
    }

    pub(crate) async fn policy_resume_choice(
        self: &Arc<Self>,
        store: String,
        service: String,
        version: String,
        proof: ChoiceProof,
    ) -> Result<Choice> {
        let scope = Scope {
            store,
            service,
            version,
        };
        let decision = self.policy_binding_for(&scope).await?;
        if proof.control != decision.control {
            return Err(fail("grant_revoked"));
        }
        if proof.authorization.is_none() {
            if !decision.allows_builtin() {
                return Err(fail("grant_revoked"));
            }
            return Ok(Choice::new(scope, None, None, decision.control));
        }
        let binding = decision.binding.ok_or(fail("grant_revoked"))?;
        let control = decision.control;
        let manager = self.clone();
        run_sync(move || {
            let context = manager.policy_context(&binding.status.package, Some(binding.clone()))?;
            manager.policy_check(&context)?;
            let choice = Choice::new(scope, None, Some(context), control);
            if choice.proof != proof {
                return Err(fail("grant_revoked"));
            }
            Ok(choice)
        })
        .await
    }
}
