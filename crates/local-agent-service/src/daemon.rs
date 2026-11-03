//! User-owned macOS launchd service and a loopback-only administrative CLI.
use local_agent_core::{providers::files, runtime};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

pub struct Options {
    pub directory: PathBuf,
    pub persistent: bool,
}
pub fn token(directory: &Path) -> Result<String, String> {
    let root = runtime::directory(directory);
    files::private_dir(&root).map_err(|e| e.message)?;
    let path = root.join("admin-token");
    if let Some(bytes) = files::read_protected(&path).map_err(|e| e.message)?.bytes {
        let value = String::from_utf8(bytes).map_err(|_| "管理令牌损坏")?;
        if value.len() < 32 || value.len() > 256 || !value.bytes().all(|b| b.is_ascii_graphic()) {
            return Err("管理令牌损坏".into());
        }
        return Ok(value);
    }
    let value = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    files::atomic_write(&path, value.as_bytes(), 0o600).map_err(|e| e.message)?;
    Ok(value)
}
fn take(map: &mut HashMap<String, String>, key: &str) -> Result<String, String> {
    map.remove(key).ok_or_else(|| format!("缺少 {key}"))
}
fn no_more(map: &HashMap<String, String>) -> Result<(), String> {
    if map.is_empty() {
        Ok(())
    } else {
        Err("存在未知或不适用的参数".into())
    }
}
fn operation_scope(
    values: &mut HashMap<String, String>,
) -> Result<Option<(String, String)>, String> {
    let operation = values.remove("--operation-id");
    let store = values.remove("--store-id");
    match (operation, store) {
        (None, None) => Ok(None),
        (Some(operation), Some(store)) if [&operation, &store].into_iter().all(|s| {
            uuid::Uuid::parse_str(s).is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == *s)
        }) => Ok(Some((operation, store))),
        _ => Err("请同时提供原 --operation-id 与 --store-id（规范 UUID v4），未自动选择新编号或当前数据库".into()),
    }
}
pub async fn command(args: Vec<String>) -> Result<Option<Options>, String> {
    if args.first().map(String::as_str) != Some("gateway") {
        if args.len() != 2 || args.first().map(String::as_str) != Some("--data-dir") {
            return Err("Expected --data-dir <path>, or gateway help".into());
        }
        return Ok(Some(Options {
            directory: PathBuf::from(&args[1]),
            persistent: false,
        }));
    }
    let verb = args.get(1).map(String::as_str).unwrap_or("help");
    if verb == "help" {
        println!(
            "local-agent-service gateway <configure|serve|install|start|stop|uninstall|status|client-add|client-revoke|client-operation|requests|providers> --data-dir <absolute-path>\nconfigure: --listen <private-ip:port> [--concurrency 16]\nclient-add: --name <name> --agent <claude|opencode|codex> --provider <id> --models <model,model> [--concurrency 4] [--operation-id <original-uuid> --store-id <original-store-uuid>]\nclient-revoke: --id <client-id> [--operation-id <original-uuid> --store-id <original-store-uuid>]\nclient-operation: --operation-id <original-uuid> --store-id <original-store-uuid>\nrequests: [--id <request-id>]\nThe default data directory is separate from the desktop service. Only first client-add returns a new downstream key. Scoped replay returns key:null; operation queries never return a key. Unscoped legacy commands have no durable operation receipt."
        );
        return Ok(None);
    }
    let mut values = HashMap::new();
    for pair in args[2..].chunks(2) {
        if pair.len() != 2
            || !pair[0].starts_with("--")
            || values.insert(pair[0].clone(), pair[1].clone()).is_some()
        {
            return Err("参数必须是唯一的 --名称 值".into());
        }
    }
    let directory = values.remove("--data-dir").map(PathBuf::from).unwrap_or(
        dirs::data_dir()
            .ok_or("无法确定数据目录")?
            .join("app.localagent.gateway"),
    );
    files::checked_path(&directory).map_err(|e| e.message)?;
    if directory.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        return Err("数据目录请使用规范绝对路径".into());
    }
    if verb == "serve" {
        no_more(&values)?;
        return Ok(Some(Options {
            directory,
            persistent: true,
        }));
    }
    if verb == "configure" {
        let config = crate::lan::Config {
            listen: take(&mut values, "--listen")?
                .parse()
                .map_err(|_| "监听地址无效")?,
            concurrency: values
                .remove("--concurrency")
                .unwrap_or("16".into())
                .parse()
                .map_err(|_| "并发数无效")?,
        };
        no_more(&values)?;
        config.validate()?;
        let _guard = runtime::ServiceGuard::acquire(&directory).map_err(|e| e.message)?;
        let root = directory.join("gateway-lan");
        files::private_dir(&root).map_err(|e| e.message)?;
        files::atomic_write(
            &root.join("config.json"),
            &serde_json::to_vec(&config).map_err(|_| "配置无效")?,
            0o600,
        )
        .map_err(|e| e.message)?;
        println!(
            "{}",
            serde_json::json!({"configured":true,"listen":config.listen,"dataDirectory":directory})
        );
        return Ok(None);
    }
    if ["install", "start", "stop", "uninstall"].contains(&verb) {
        no_more(&values)?;
        manage(verb, &directory).await?;
        return Ok(None);
    }
    let (method, route, body) = match verb {
        "status" => (reqwest::Method::GET, "/api/gateway-lan".into(), None),
        "providers" => (reqwest::Method::GET, "/api/snapshot".into(), None),
        "client-add" => {
            let body = serde_json::json!({"name":take(&mut values,"--name")?,"agent":take(&mut values,"--agent")?,
                "providerId":take(&mut values,"--provider")?,"models":take(&mut values,"--models")?.split(',').collect::<Vec<_>>(),
                "concurrency":values.remove("--concurrency").unwrap_or("4".into()).parse::<usize>().map_err(|_|"并发数无效")?});
            if let Some((operation_id, store_id)) = operation_scope(&mut values)? {
                no_more(&values)?;
                (
                    reqwest::Method::POST,
                    "/api/gateway-lan/operations".into(),
                    Some(
                        serde_json::json!({"storeId":store_id,"operationId":operation_id,"action":{"kind":"create","draft":body}}),
                    ),
                )
            } else {
                (
                    reqwest::Method::POST,
                    "/api/gateway-lan/clients".into(),
                    Some(body),
                )
            }
        }
        "client-revoke" => {
            let id = take(&mut values, "--id")?;
            uuid::Uuid::parse_str(&id).map_err(|_| "客户端 ID 无效")?;
            if let Some((operation_id, store_id)) = operation_scope(&mut values)? {
                (
                    reqwest::Method::POST,
                    "/api/gateway-lan/operations".into(),
                    Some(
                        serde_json::json!({"storeId":store_id,"operationId":operation_id,"action":{"kind":"revoke","clientId":id}}),
                    ),
                )
            } else {
                (
                    reqwest::Method::POST,
                    format!("/api/gateway-lan/clients/{id}/revoke"),
                    None,
                )
            }
        }
        "client-operation" => {
            let (operation, store) =
                operation_scope(&mut values)?.ok_or("请提供原操作编号与数据库身份")?;
            (
                reqwest::Method::GET,
                format!("/api/gateway-lan/operations/{operation}?storeId={store}"),
                None,
            )
        }
        "requests" => {
            let route = if let Some(id) = values.remove("--id") {
                format!(
                    "/api/request-detail?{}",
                    url::form_urlencoded::Serializer::new(String::new())
                        .append_pair("id", &id)
                        .finish()
                )
            } else {
                "/api/requests".into()
            };
            (reqwest::Method::GET, route, None)
        }
        _ => return Err("未知网关命令，请使用 gateway help".into()),
    };
    no_more(&values)?;
    let mut value = request(&directory, method, &route, body).await?;
    if verb == "providers" {
        value = value["providers"].take();
    }
    if verb == "status" {
        let record = runtime::read_service(&directory)?.ok_or("服务未运行")?;
        value["persistent"] = record.identity.persistent.into();
        value["pid"] = record.identity.pid.into();
        value["dataDirectory"] = directory.to_string_lossy().into_owned().into();
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|_| "结果无法序列化")?
    );
    Ok(None)
}
async fn request(
    directory: &Path,
    method: reqwest::Method,
    route: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let record = runtime::read_service(directory)?.ok_or("服务未运行")?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "无法建立本机连接")?;
    let base = format!("http://127.0.0.1:{}", record.port);
    let identity: runtime::Identity = client
        .get(format!("{base}/api/runtime"))
        .bearer_auth(&record.token)
        .send()
        .await
        .map_err(|_| "无法连接已发布服务")?
        .error_for_status()
        .map_err(|_| "服务身份验证失败")?
        .json()
        .await
        .map_err(|_| "服务身份无效")?;
    if identity != record.identity
        || identity.version != env!("CARGO_PKG_VERSION")
        || identity.protocol != runtime::PROTOCOL
    {
        return Err("服务身份或版本不匹配，未发送管理操作".into());
    }
    let mut request = client
        .request(method, format!("{base}{route}"))
        .bearer_auth(&record.token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.map_err(|_| "本机管理请求失败")?;
    let status = response.status();
    let value: serde_json::Value = response.json().await.map_err(|_| "管理响应无效")?;
    if !status.is_success() {
        return Err(value["message"].as_str().unwrap_or("管理操作失败").into());
    }
    Ok(value)
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn launch(label: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new("/bin/launchctl")
        .args(args)
        .output()
        .map_err(|_| "无法调用 macOS 服务管理器")?;
    if !output.status.success() {
        return Err(format!("macOS 服务操作失败：{label}（{}）", output.status));
    }
    Ok(())
}
async fn manage(verb: &str, directory: &Path) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("常驻安装目前仅支持 macOS".into());
    }
    // The label and file are scoped to exactly one data directory; never enumerate or stop other jobs.
    let label = format!(
        "app.localagent.gateway.{}",
        &files::hash(directory.as_os_str().as_encoded_bytes())[..16]
    );
    let home = dirs::home_dir().ok_or("无法确定用户目录")?;
    let agents = home.join("Library/LaunchAgents");
    let plist = agents.join(format!("{label}.plist"));
    let uid = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .map_err(|_| "无法确定用户 ID")?;
    if !uid.status.success() {
        return Err("无法确定用户 ID".into());
    }
    let uid = String::from_utf8(uid.stdout).map_err(|_| "用户 ID 无效")?;
    let domain = format!("gui/{}", uid.trim());
    let target = format!("{domain}/{label}");
    let program = directory.join("service/local-agent-service");
    if verb == "install" {
        let _guard = runtime::ServiceGuard::acquire(directory).map_err(|e| e.message)?;
        let lan = crate::lan::LanGateway::open(directory)?;
        if lan.config.is_none() {
            return Err("请先运行 gateway configure 设置独立监听地址".into());
        }
        if plist.exists() {
            return Err("此数据目录已安装常驻服务；请先停止并卸载再更新".into());
        }
        files::private_dir(program.parent().ok_or("服务目录无效")?).map_err(|e| e.message)?;
        files::checked_path(&program).map_err(|e| e.message)?;
        let mut source =
            std::fs::File::open(std::env::current_exe().map_err(|_| "无法定位服务程序")?)
                .map_err(|_| "无法读取服务程序")?;
        // create_new avoids overwriting an executable that an existing process might use.
        if program.exists() {
            std::fs::remove_file(&program).map_err(|_| "无法替换停止的服务副本")?;
        }
        use std::os::unix::fs::OpenOptionsExt;
        let mut destination = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&program)
            .map_err(|_| "无法创建独立服务程序")?;
        std::io::copy(&mut source, &mut destination)
            .and_then(|_| destination.sync_all())
            .map_err(|_| "无法保存独立服务程序")?;
        let logs = directory.join("logs");
        files::private_dir(&logs).map_err(|e| e.message)?;
        for name in ["service.log", "service-error.log"] {
            files::atomic_write(&logs.join(name), b"", 0o600).map_err(|e| e.message)?;
        }
        files::checked_path(&agents).map_err(|e| e.message)?;
        std::fs::create_dir_all(&agents).map_err(|_| "无法创建用户服务目录")?;
        let mut environment = String::new();
        // No ambient proxy or shared Agent configuration is changed. Test environments stay scoped.
        for key in [
            "LOCAL_AGENT_SOURCE_HOME",
            "LOCAL_AGENT_TEST_MEMORY_VAULT",
            "LOCAL_AGENT_TEST_IDLE_AGENTS",
        ] {
            if let Ok(value) = std::env::var(key) {
                environment.push_str(&format!(
                    "<key>{}</key><string>{}</string>",
                    xml(key),
                    xml(&value)
                ));
            }
        }
        let contents = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array><string>{}</string><string>gateway</string><string>serve</string><string>--data-dir</string><string>{}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict><key>ThrottleInterval</key><integer>10</integer><key>EnvironmentVariables</key><dict>{environment}</dict><key>StandardOutPath</key><string>{}</string><key>StandardErrorPath</key><string>{}</string><key>ProcessType</key><string>Background</string></dict></plist>",
            xml(&program.to_string_lossy()),
            xml(&directory.to_string_lossy()),
            xml(&logs.join("service.log").to_string_lossy()),
            xml(&logs.join("service-error.log").to_string_lossy())
        );
        files::atomic_write(&plist, contents.as_bytes(), 0o600).map_err(|e| e.message)?;
        drop(_guard);
    }
    let saved = files::read_protected(&plist)
        .map_err(|e| e.message)?
        .bytes
        .ok_or("此目录未安装常驻服务")?;
    let saved = String::from_utf8(saved).map_err(|_| "常驻定义无效")?;
    if !saved.contains(&format!(
        "<string>{}</string>",
        xml(&program.to_string_lossy())
    )) || !saved.contains(&format!(
        "<string>{}</string>",
        xml(&directory.to_string_lossy())
    )) {
        return Err("常驻服务定义身份不匹配，未执行操作".into());
    }
    let loaded = Command::new("/bin/launchctl")
        .args(["print", &target])
        .output()
        .map_err(|_| "无法检查用户服务")?
        .status
        .success();
    if verb == "stop" || verb == "uninstall" {
        if loaded {
            launch(&label, &["bootout", &target])?;
        }
        for _ in 0..180 {
            let running = runtime::read_service(directory)?.is_some();
            if !running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if runtime::read_service(directory)?.is_some() {
            return Err("服务仍在排空请求，未清理定义或数据".into());
        }
        if verb == "uninstall" {
            std::fs::remove_file(&plist).map_err(|_| "无法删除所属常驻定义")?;
        }
        println!(
            "{}",
            serde_json::json!({"stopped":true,"uninstalled":verb=="uninstall","label":label})
        );
    } else {
        if loaded {
            launch(&label, &["kickstart", &target])?;
        } else {
            launch(&label, &["bootstrap", &domain, &plist.to_string_lossy()])?;
        }
        let mut ready = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            if tokio::time::timeout(
                Duration::from_millis(700),
                request(directory, reqwest::Method::GET, "/api/runtime", None),
            )
            .await
            .is_ok_and(|result| result.is_ok())
            {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if !ready {
            return Err("常驻服务尚未就绪，请检查此数据目录的 logs".into());
        }
        println!(
            "{}",
            serde_json::json!({"running":true,"label":label,"dataDirectory":directory})
        );
    }
    Ok(())
}
