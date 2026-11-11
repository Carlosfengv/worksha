use super::*;
use crate::{
    agents::{AgentDirectory, ProductDescriptor, SourceDescriptor},
    sessions::{CallQuery, SessionQuery, opencode, source_plugins::test_fixture::Fixture},
};
const AGENT: &str = "fixture-agent";
struct FixtureSource;
fn remap(summary: &mut super::super::SessionSummary) {
    summary.id = summary.id.replacen("opencode:", "fixture-agent:", 1);
    summary.agent_id = AGENT.into();
    summary.agent_product_id = Some(AGENT.into());
    summary.agent_products = vec![AGENT.into()];
    summary.has_unknown_client = false;
}
impl BuiltinPlugin for FixtureSource {
    fn descriptor(&self) -> crate::plugin::PluginDescriptor {
        descriptor(AGENT, "io.localagent.test.fixture")
    }
    fn prepare(&self) -> Result<crate::plugin::PluginResources, crate::plugin::PluginError> {
        Ok(Default::default())
    }
}
impl ReadOnlySessionSource for FixtureSource {
    fn agent(&self) -> &'static str {
        AGENT
    }
    fn scan(
        &self,
        context: &SourceContext<'_>,
        previous: &HashMap<PathBuf, IndexedFile>,
    ) -> NativeResult<SourceScan> {
        let mut result = opencode::OpenCodeSource.scan(context, previous)?;
        for file in result.files.values_mut() {
            file.agent = AGENT.into();
            remap(&mut file.summary);
            file.native_products = vec![AGENT.into()];
            for call in Arc::make_mut(&mut file.calls) {
                call.agent_id = AGENT.into();
                call.agent_product_id = Some(AGENT.into());
                call.native_agent_product_id = Some(AGENT.into());
                call.id = call.id.replacen("opencode:", "fixture-agent:", 1);
                call.session_id = file.summary.id.clone();
            }
            file.fact_calls = super::super::domain_facts::CallIndex::new(&file.calls);
        }
        Ok(result)
    }
    fn conversation(
        &self,
        context: &SourceContext<'_>,
        indexed: &IndexedFile,
        cursor: Option<&str>,
        anchor: Option<&str>,
    ) -> NativeResult<Conversation> {
        let mut result = opencode::OpenCodeSource.conversation(context, indexed, cursor, anchor)?;
        remap(&mut result.session);
        for event in &mut result.items {
            event.agent_product_id = Some(AGENT.into());
        }
        Ok(result)
    }
}

#[test]
fn new_shared_family_keeps_product_groups_visible_under_family_and_unknown_filters() {
    let fixture = Fixture::new();
    let sessions = NativeSessions::new(fixture.home.clone(), &["opencode".into()]);
    sessions.refresh().unwrap();
    let mut catalog = sessions.project_catalog().unwrap();
    let directory = Arc::new(
        AgentDirectory::new(
            vec![SourceDescriptor {
                id: AGENT.into(),
                name: "Shared Fixture".into(),
                shared: true,
            }],
            ["fixture-cli", "fixture-desktop"]
                .into_iter()
                .map(|id| ProductDescriptor {
                    id: id.into(),
                    name: id.into(),
                    family: AGENT.into(),
                    config_id: AGENT.into(),
                    locations: vec![],
                })
                .collect(),
        )
        .unwrap(),
    );
    for session in &mut catalog.sessions {
        session.agent_id = AGENT.into();
        session.agent_product_id = Some("fixture-cli".into());
        session.agent_products = vec!["fixture-cli".into()];
        session.has_unknown_client = false;
    }
    for call in catalog.calls.iter_mut() {
        call.agent_id = AGENT.into();
        call.agent_product_id = Some("fixture-cli".into());
    }
    catalog.calls = catalog.calls.with_directory(directory);
    for (agent, expected) in [
        (AGENT, 1),
        ("fixture-cli", 1),
        ("fixture-desktop", 0),
        ("fixture-agent-unknown", 0),
    ] {
        let page = crate::projects::query(
            &catalog,
            crate::projects::ProjectQuery {
                agent: Some(agent.into()),
                scope: Some("all".into()),
                ..Default::default()
            },
            &Default::default(),
        )
        .unwrap();
        assert_eq!(page.summary.model_calls, expected, "{agent}");
        if expected > 0 {
            assert_eq!(page.items[0].agents[0].agent_id, "fixture-cli");
        }
    }
}
#[test]
fn added_descriptor_flows_through_storage_directory_sessions_and_all_read_models() {
    let fixture = Fixture::new();
    let data = tempfile::tempdir().unwrap();
    let now = chrono::DateTime::from_timestamp(1_790_000_100, 0).unwrap();
    let builtins = crate::agents::builtin_directory();
    let mut sources = builtins.sources().to_vec();
    sources.push(SourceDescriptor {
        id: AGENT.into(),
        name: "Fixture Agent".into(),
        shared: false,
    });
    let mut products = builtins.products().to_vec();
    products.push(ProductDescriptor {
        id: AGENT.into(),
        family: AGENT.into(),
        config_id: AGENT.into(),
        name: "Fixture Agent".into(),
        locations: vec![],
    });
    let directory = Arc::new(AgentDirectory::new(sources, products).unwrap());
    assert!(directory.valid_filter(AGENT));
    assert!(!directory.valid_filter("unregistered-fixture"));
    let store = crate::Store::open_with_directory(data.path(), directory.clone()).unwrap();
    store.save_native_source(AGENT, true).unwrap();
    assert_eq!(
        store.snapshot().unwrap().agents.last().unwrap().name,
        "Fixture Agent"
    );
    let mut plugins = NativeSourcePlugins::builtins();
    plugins.push(Arc::new(FixtureSource));
    let plugins = NativeSourcePlugins::mount_with_directory(plugins, directory).unwrap();
    let sessions = NativeSessions::with_source_plugins(
        fixture.home.clone(),
        &store.native_sources().unwrap(),
        plugins,
    );
    sessions.refresh().unwrap();
    let page = sessions
        .query(SessionQuery {
            agent: Some(AGENT.into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.summary.unwrap().usage.total_tokens, Some(120));
    assert_eq!(
        sessions
            .conversation(&page.items[0].id, None, None)
            .unwrap()
            .session
            .agent_id,
        AGENT
    );
    assert_eq!(
        sessions
            .calls(CallQuery {
                agent: Some(AGENT.into()),
                ..Default::default()
            })
            .unwrap()
            .total,
        2
    );
    assert_eq!(
        sessions
            .mcp(super::super::McpQuery {
                agent: Some(AGENT.into()),
                ..Default::default()
            })
            .unwrap()
            .total_calls,
        1
    );
    let catalog = sessions.project_catalog().unwrap();
    let projects = crate::projects::query(
        &catalog,
        crate::projects::ProjectQuery {
            agent: Some(AGENT.into()),
            scope: Some("all".into()),
            ..Default::default()
        },
        &Default::default(),
    )
    .unwrap();
    assert_eq!(projects.summary.usage.total_tokens, Some(120));
    let fees = crate::associated_cost::Index::new(&catalog, vec![], false);
    assert!(
        fees.details(
            &[page.items[0].id.clone()],
            &crate::associated_cost::detail::Query {
                scope: "project".into(),
                id: projects.items[0].identity.id.clone(),
                agent: Some(AGENT.into()),
                ..Default::default()
            }
        )
        .is_ok()
    );
    let mut sql = crate::observation::sql::History::open(data.path()).unwrap();
    sql.use_directory(sessions.directory()).unwrap();
    assert_eq!(
        sql.model_usage(
            crate::model_usage::UsageQuery {
                agent: AGENT.into(),
                source: "gateway".into(),
                days: None,
                provider: None,
                model: None,
                cursor: None
            },
            "fixture".into(),
            "ready".into(),
            now
        )
        .unwrap()["summary"]["attempts"],
        0
    );
    sql.refresh(&[]).unwrap();
    sql.sync_native_provider(&catalog, &[]).unwrap();
    sql.sync_native_models(&catalog, AGENT).unwrap();
    let providers = sql
        .provider_usage(
            crate::provider_usage::Query {
                source: "native".into(),
                agent: Some(AGENT.into()),
                ..Default::default()
            },
            &[],
            "fixture".into(),
            "ready".into(),
            chrono::Utc::now(),
        )
        .unwrap();
    assert_eq!(providers["summary"]["calls"], 1);
    let models = sql
        .model_usage(
            crate::model_usage::UsageQuery {
                agent: AGENT.into(),
                source: "native".into(),
                days: None,
                provider: None,
                model: None,
                cursor: None,
            },
            "fixture".into(),
            "ready".into(),
            chrono::Utc::now(),
        )
        .unwrap();
    assert_eq!(models["summary"]["usage"]["totalTokens"], 120);
    sql.sync_native_analysis(&catalog).unwrap();
    let analysis = sql
        .analysis(
            &crate::observation::OverviewQuery {
                agent: Some(AGENT.into()),
                range: 168,
                metric: "tokens".into(),
                dimension: "agent".into(),
                source: "native".into(),
            },
            now,
        )
        .unwrap();
    assert_eq!(analysis["total"].as_f64(), Some(120.));
    sql.sync_native_catalog(
        catalog.revision.to_string(),
        &HashMap::new(),
        &catalog.calls,
    )
    .unwrap();
    let query = crate::requests::Query {
        source: Some("native".into()),
        agent: Some(AGENT.into()),
        ..Default::default()
    };
    let report = sql
        .source_report(&query, now, sessions.sources().unwrap(), None)
        .unwrap();
    assert_eq!(report["native"]["requests"], 1);
    assert_eq!(report["collectors"].as_array().unwrap().len(), 1);
    let requests = sql.request_page(&query, now, report, None).unwrap();
    assert_eq!(requests["total"], 1);
    sessions.stop_source_plugin(AGENT).unwrap();
    assert_eq!(
        sessions
            .query(SessionQuery {
                agent: Some(AGENT.into()),
                ..Default::default()
            })
            .unwrap()
            .total,
        0
    );
    assert_eq!(store.native_sources().unwrap(), vec![AGENT]);
    drop(store);
    // Older compositions safely retain the stored grant without activating an unknown implementation.
    let old = crate::Store::open(data.path()).unwrap();
    assert_eq!(old.native_sources().unwrap(), vec![AGENT]);
    let legacy = NativeSessions::new(fixture.home.clone(), &old.native_sources().unwrap());
    legacy.refresh().unwrap();
    assert_eq!(legacy.query(SessionQuery::default()).unwrap().total, 0);
}
