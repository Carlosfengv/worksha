mod backend;
mod commands;
mod instance;
#[cfg(target_os = "macos")]
mod menu;
use backend::Backend;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();
    let data = match local_agent_core::runtime::desktop_data_dir(&context.config().identifier) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("{error}");
            return;
        }
    };
    let _owner = match instance::acquire(&data, &context.config().identifier) {
        Ok(Some(owner)) => owner,
        Ok(None) => return,
        Err(error) => {
            eprintln!("{error}");
            return;
        }
    };
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .setup(backend::start)
        .invoke_handler(commands::iam::guard_invocations(tauri::generate_handler![
            commands::iam::get_iam_status,
            commands::iam::perform_iam,
            commands::object_contributions::perform_object_contribution,
            commands::routing_policy::perform_routing_policy,
            commands::routing_policy::perform_routing_policy_recovery,
            commands::provider_catalog::query_provider_catalog,
            commands::provider_catalog::perform_provider_catalog,
            commands::provider_catalog::commit_catalog_provider_operation,
            commands::provider_catalog::get_catalog_provider_operation,
            commands::provider_catalog::perform_scoped_provider_catalog,
            commands::provider_catalog::perform_provider_catalog_source,
            commands::provider_catalog::perform_provider_source_operation,
            commands::provider_catalog::perform_provider_draft_catalog,
            commands::provider_catalog::perform_provider_draft_grant,
            commands::provider_catalog::perform_provider_draft_recovery,
            commands::provider_catalog::query_provider_presets,
            commands::domain::query_domain_catalog,
            commands::domain::query_unified_facts,
            commands::domain::query_unified_activity,
            commands::domain::query_unified_requests,
            commands::requests::export_unified_request_csv,
            commands::domain::get_fact_conversation,
            commands::domain::set_domain_project_visibility,
            commands::domain::perform_agent_model_connection,
            commands::domain::list_full_plugins,
            commands::domain::inspect_full_plugin_package,
            commands::domain::install_full_plugin_package,
            commands::domain::change_full_plugin_state,
            commands::domain::query_domain_capabilities,
            commands::domain::authorize_domain_source,
            commands::domain::refresh_domain_source,
            commands::domain::query_unified_sessions,
            commands::domain::get_unified_conversation,
            commands::domain::set_domain_source_consent,
            commands::domain::prepare_domain_source_consent,
            commands::plugins::list_plugins,
            commands::plugins::get_builtin_plugins,
            commands::plugins::configure_builtin_plugins,
            commands::plugins::get_builtin_plugin_operation,
            commands::plugins::inspect_plugin_package,
            commands::plugins::inspect_plugin_upload,
            commands::plugins::install_plugin_package,
            commands::plugins::change_plugin_state,
            commands::plugins::authorize_plugin_source,
            commands::plugins::query_plugin_view,
            commands::plugins::perform_plugin_action,
            commands::plugins::query_plugin_extension,
            commands::plugins::perform_plugin_extension,
            commands::plugins::authorize_plugin_resource,
            commands::plugins::export_plugin_connector,
            commands::accounts::get_accounts,
            commands::accounts::refresh_account,
            commands::accounts::create_claude_quota_launcher,
            commands::accounts::open_account_console,
            commands::settings::get_snapshot,
            commands::backend::get_backend_diagnostics,
            commands::backend::reconnect_backend,
            commands::settings::save_settings,
            commands::settings::preview_retention,
            commands::settings::preview_retention_inventory,
            commands::settings::get_settings_operation,
            commands::settings::get_scoped_settings_operation,
            commands::settings::save_provider,
            commands::gateway::get_gateway_status,
            commands::gateway::create_gateway_client,
            commands::gateway::revoke_gateway_client,
            commands::gateway::get_gateway_client_operation,
            commands::gateway::commit_gateway_client_operation,
            commands::gateway::save_gateway_client_operation,
            commands::providers::get_provider_status,
            commands::local_tokens::get_local_token_protocol,
            commands::local_tokens::get_local_token_operation,
            commands::local_tokens::continue_local_token_operation,
            commands::local_tokens::cancel_local_token_operation,
            commands::model_services::get_model_services,
            commands::model_services::commit_model_service_operation,
            commands::model_services::get_model_service_operation,
            commands::model_services::verify_model_service_member,
            commands::model_services::save_model_service,
            commands::model_services::delete_model_service,
            commands::model_services::prepare_model_service_connection,
            commands::providers::query_provider_usage,
            commands::providers::discover_provider_imports,
            commands::providers::import_native_provider,
            commands::native_imports::preview_native_provider_import,
            commands::native_imports::get_native_provider_import,
            commands::native_imports::cancel_native_provider_import,
            commands::providers::create_provider_credential,
            commands::providers::get_provider_credential_protocol,
            commands::credential_recovery::get_provider_credential_protocol_v3,
            commands::credential_recovery::initialize_provider_credential_namespace,
            commands::credential_recovery::create_provider_credential_v3,
            commands::credential_recovery::get_provider_credential_operation_v3,
            commands::credential_recovery::cancel_provider_credential_operation_v3,
            commands::credential_recovery::preview_legacy_provider_credential,
            commands::credential_recovery::discover_provider_credentials,
            commands::credential_recovery::confirm_legacy_provider_credential,
            commands::providers::create_scoped_provider_credential,
            commands::providers::get_scoped_provider_credential_operation,
            commands::providers::cancel_scoped_provider_credential_operation,
            commands::providers::get_provider_credential_operation,
            commands::providers::cancel_provider_credential_operation,
            commands::providers::verify_provider,
            commands::providers::connect_provider,
            commands::providers::get_provider_write_protocol,
            commands::providers::commit_provider_operation,
            commands::providers::get_provider_operation,
            commands::providers::cancel_provider_operation,
            commands::providers::fetch_provider_models,
            commands::providers::prepare_provider_switch,
            commands::providers::prepare_dynamic_connection,
            commands::providers::prepare_dynamic_route,
            commands::providers::dynamic_route_action,
            commands::providers::provider_switch_action,
            commands::provider_restart::restart_agent_provider,
            commands::sessions::query_sessions,
            commands::sessions::query_associated_costs,
            commands::sessions::query_calls,
            commands::sessions::query_mcp,
            commands::sessions::get_conversation,
            commands::sessions::query_agent_memory,
            commands::sessions::set_session_source,
            commands::sessions::refresh_sessions,
            commands::observation::query_overview,
            commands::observation::query_agent_usage,
            commands::observation::query_requests,
            commands::requests::query_request_dashboard,
            commands::requests::get_request_detail,
            commands::requests::export_request_csv,
            commands::observation::save_observation_binding,
            commands::projects::query_projects,
            commands::workbench::get_workbench_routes,
            commands::workbench::get_workbench_shared_routes,
            commands::workbench::create_workbench_shared,
            commands::workbench::read_workbench_tasks,
            commands::workbench::read_workbench_operation,
            commands::workbench::find_workbench_session,
            commands::workbench::create_workbench_task,
            commands::workbench::send_workbench_message,
            commands::workbench::control_workbench_turn,
            commands::workbench::open_workbench_desktop,
            commands::workbench::export_workbench_opencode,
            commands::workbench::read_workbench_live,
            commands::workbench::read_workbench_live_board,
            commands::workbench::control_workbench_live,
            commands::projects::set_project_hidden,
            commands::projects::get_project,
            commands::projects::get_project_diff,
            commands::projects::select_project_directory
        ]))
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        });
    #[cfg(target_os = "macos")]
    let builder = builder.menu(menu::create).on_menu_event(|app, event| {
        if event.id().as_ref() == menu::QUIT {
            app.exit(0);
        }
    });
    let app = builder.build(context).expect("无法启动 Local Agent");
    let stopping = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    app.run(move |app, event| match event {
        tauri::RunEvent::ExitRequested { api, code, .. } => {
            use std::sync::atomic::Ordering;
            // Shell cleanup kills sidecars at Exit. Delay that event while a
            // worker drains the service; the native main thread stays responsive.
            if !stopped.load(Ordering::Acquire) {
                api.prevent_exit();
                if !stopping.swap(true, Ordering::AcqRel) {
                    let app = app.clone();
                    let stopped = stopped.clone();
                    let data = data.clone();
                    std::thread::spawn(move || {
                        if let Some(backend) = app.try_state::<Backend>() {
                            backend.stop(&data);
                        }
                        stopped.store(true, Ordering::Release);
                        app.exit(code.unwrap_or(0));
                    });
                }
            }
        }
        tauri::RunEvent::Exit => {
            // macOS can also terminate directly from the Dock or system. There
            // is no remaining interactive work to unblock at this final event.
            if !stopped.load(std::sync::atomic::Ordering::Acquire)
                && let Some(backend) = app.try_state::<Backend>()
            {
                backend.stop(&data);
            }
        }
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        _ => {}
    });
}
