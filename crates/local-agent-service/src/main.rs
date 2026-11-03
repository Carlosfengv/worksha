mod api;
mod associated_costs;
mod billing;
mod blocking;
mod connections;
mod daemon;
mod domain;
mod gateway;
mod lan;
mod local_tokens;
mod model_services;
mod model_usage;
mod observation;
mod plugin_host;
mod projects;
mod provider_connect;
mod provider_models;
mod provider_usage;
mod providers;
mod routes;
mod runtime;
mod sessions;
mod settings;
mod state;
mod tasks;
use api::{ApiResult, error, json_value, native_error};
use local_agent_core::Store;
use state::AppState;
use std::{
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--plugin-worker") {
        let path = PathBuf::from(std::env::args().nth(2).ok_or("Missing package")?);
        let digest = std::env::args().nth(3).ok_or("Missing digest")?;
        if let Err(error) = plugin_host::external::worker::run(&path, &digest) {
            eprintln!("plugin-worker: {}", error.code);
            std::process::exit(1);
        }
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--plugin-sdk") {
        plugin_host::external::sdk::run(std::env::args().skip(2).collect())?;
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(service_main())
}
async fn service_main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--read-mcp-config") {
        let path =
            std::path::PathBuf::from(std::env::args().nth(2).ok_or("Missing MCP config path")?);
        match local_agent_core::sessions::read_config_worker(&path) {
            Ok(value) => {
                std::io::stdout().write_all(&serde_json::to_vec(&value)?)?;
                return Ok(());
            }
            Err(_) => std::process::exit(1),
        }
    }
    local_agent_core::sessions::install_reader_worker(std::env::current_exe()?);
    if std::env::args().nth(1).as_deref() == Some("--vault-worker") {
        local_agent_core::providers::vault::run_worker()?;
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--claude-quota-stdin") {
        let directory = PathBuf::from(std::env::args().nth(2).ok_or("Missing quota directory")?);
        billing::capture_stdin(&directory)?;
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--credential") {
        use local_agent_core::providers::vault::Vault;
        let reference = std::env::args()
            .nth(2)
            .ok_or("Missing credential reference")?;
        uuid::Uuid::parse_str(&reference).map_err(|_| "Invalid credential reference")?;
        let secret = local_agent_core::providers::vault::SystemVault
            .get(&reference)
            .map_err(|_| "Credential unavailable")?;
        std::io::stdout().write_all(&secret)?;
        return Ok(());
    }
    let Some(options) = daemon::command(std::env::args().skip(1).collect()).await? else {
        return Ok(());
    };
    let directory = options.directory;
    let runtime_guard = local_agent_core::runtime::ServiceGuard::acquire(&directory)
        .map_err(|error| error.message)?;
    #[cfg(not(debug_assertions))]
    if std::env::var_os("LOCAL_AGENT_TEST_MEMORY_VAULT").is_some()
        || std::env::var_os("LOCAL_AGENT_TEST_IDLE_AGENTS").is_some()
    {
        return Err("Test fixtures require the dedicated debug test service".into());
    }
    let token = if options.persistent {
        daemon::token(&directory)?
    } else {
        std::env::var("LOCAL_AGENT_SERVICE_TOKEN").map_err(|_| "Missing service token")?
    };
    if token.len() < 32 {
        return Err("Invalid service token".into());
    }
    // Also respect the gateway lock held by older bundles before opening stores
    // or installing helpers; their pre-discovery service cannot be adopted.
    let (gateway, gateway_listener) = gateway::Gateway::bind(&directory).await?;
    let lan_listener = if options.persistent {
        gateway.lan.bind().await?
    } else {
        if gateway.lan.config.is_some() {
            return Err("此目录属于常驻网关，请通过 gateway start 启动".into());
        }
        None
    };
    if directory
        == dirs::home_dir()
            .ok_or("Missing home")?
            .join("Library/Application Support/app.localagent.desktop")
    {
        local_agent_core::providers::vault::install_worker(&std::env::current_exe()?)
            .map_err(|e| e.message)?;
    }
    let store = Store::open(&directory)?;
    gateway.validate_token_owners(&store)?;
    gateway
        .lan
        .ensure_store(&store.database_id().map_err(|e| e.message)?)?;
    let source_home = std::env::var_os("LOCAL_AGENT_SOURCE_HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .ok_or("Missing user home")?;
    let provider_vault: Arc<dyn local_agent_core::providers::vault::Vault> = {
        #[cfg(debug_assertions)]
        if std::env::var("LOCAL_AGENT_TEST_MEMORY_VAULT").as_deref() == Ok("1") {
            if !source_home.starts_with(std::env::temp_dir().canonicalize()?)
                || source_home == dirs::home_dir().unwrap_or_default()
            {
                return Err("Test vault requires an isolated temporary source home".into());
            }
            Arc::new(local_agent_core::providers::vault::MemoryVault::default())
        } else {
            Arc::new(local_agent_core::providers::vault::SystemVault)
        }
        #[cfg(not(debug_assertions))]
        {
            Arc::new(local_agent_core::providers::vault::SystemVault)
        }
    };
    let provider_manager = local_agent_core::providers::ProviderManager::new(
        source_home.clone(),
        &directory,
        std::env::current_exe()?,
        provider_vault.clone(),
    )
    .map_err(|e| e.message)?;
    let credential_store_id = store.database_id().map_err(|e| e.message)?;
    let credential_scope = store
        .credential_scope(&credential_store_id)
        .map_err(|e| e.message)?;
    provider_manager
        .bind_legacy_credential_source(&credential_scope)
        .map_err(|e| e.message)?;
    let readonly = Arc::new(plugin_host::ReadonlyRuntime::builtin()?);
    let billing = Arc::new(Mutex::new(billing::Billing::with_sources(
        source_home.clone(),
        &directory,
        readonly.accounts.clone(),
    )));
    let sessions = Arc::new(plugin_host::sessions(
        source_home,
        &store.native_sources()?,
    )?);
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let address = listener.local_addr()?;
    let state = AppState {
        provider_adapters: provider_models::adapters::builtin(),
        provider_client: state::provider_client().map_err(|_| "无法建立模型连接服务")?,
        credentials: provider_vault.clone(),
        plugins: Arc::new(plugin_host::external::Manager::for_service(
            &directory,
            provider_vault,
        )),
        readonly: readonly.clone(),
        billing,
        gateway: Arc::new(gateway),
        store: Arc::new(Mutex::new(store)),
        token: Arc::new(token),
        host: Arc::new(address.to_string()),
        sessions,
        session_work: Arc::new(sessions::work::SessionWork::default()),
        providers: Arc::new(Mutex::new(provider_manager)),
        verification_capacity: Arc::new(tokio::sync::Semaphore::new(2)),
        query_capacity: blocking::query_capacity(),
        listeners: Arc::new(Mutex::new(Default::default())),
        observation: Arc::new(observation::Observation::new(
            &directory,
            &address.to_string(),
        )?),
    };
    let identity = runtime_guard.publish_mode(
        address.port(),
        state.token.as_ref().clone(),
        env!("CARGO_PKG_VERSION"),
        options.persistent,
    )?;
    let runtime = runtime::Runtime::new(identity);
    let app = routes::management(state.clone()).merge(runtime.routes(state.clone()));
    if !options.persistent {
        tasks::start(&state);
    }
    println!(
        "{}",
        serde_json::json!({"port":address.port(), "version":env!("CARGO_PKG_VERSION")})
    );
    std::io::stdout().flush()?;
    let observation = state.observation.clone();
    let source_plugins = state.sessions.clone();
    let session_work = state.session_work.clone();
    let external_plugins = state.plugins.clone();
    let lan_app = routes::lan(state.clone());
    let gateway_app = routes::gateway(state);
    let signal_runtime = runtime.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut termination) => {
                    tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = termination.recv() => {} }
                }
                Err(_) => {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
        signal_runtime.stop();
    });
    let servers = async {
        tokio::try_join!(
            axum::serve(gateway_listener, gateway_app).with_graceful_shutdown({
                let runtime = runtime.clone();
                async move { runtime.stopped().await }
            }),
            axum::serve(listener, app).with_graceful_shutdown({
                let runtime = runtime.clone();
                async move { runtime.stopped().await }
            }),
            async {
                if let Some(listener) = lan_listener {
                    axum::serve(
                        listener,
                        lan_app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                    )
                    .with_graceful_shutdown({
                        let runtime = runtime.clone();
                        async move { runtime.stopped().await }
                    })
                    .await
                } else {
                    Ok(())
                }
            },
        )
    };
    let stop_readers = || {
        session_work.stop();
        external_plugins.stop();
        readonly.stop();
        if let Err(error) = source_plugins.stop_source_plugins() {
            eprintln!("{}", error.message);
        }
    };
    let draining_at = {
        tokio::pin!(servers);
        tokio::select! {
            result = &mut servers => { let started = std::time::Instant::now(); stop_readers(); result?; started },
            _ = runtime.stopped() => {
                let started = std::time::Instant::now(); stop_readers();
                let remaining = std::time::Duration::from_secs(10).saturating_sub(started.elapsed());
                match tokio::time::timeout(remaining, &mut servers).await {
                    Ok(result) => { result?; },
                    Err(_) => observation.interrupt_active(),
                }
                started
            }
        }
    };
    let remaining = std::time::Duration::from_secs(10).saturating_sub(draining_at.elapsed());
    if !readonly.collectors.drain(remaining).await {
        eprintln!("只读采集器已取消，排空预算已用尽");
    }
    let remaining = std::time::Duration::from_secs(10).saturating_sub(draining_at.elapsed());
    if !session_work.drain(remaining).await {
        eprintln!("会话清理工作尚未排空，来源读取已停止");
    }
    // Listener completion is separate from the durable metadata acknowledgement.
    if let Err(message) = tokio::task::spawn_blocking(move || observation.flush()).await? {
        eprintln!("{message}");
    }
    Ok(())
}
