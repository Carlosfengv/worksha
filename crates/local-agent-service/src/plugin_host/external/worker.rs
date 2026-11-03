use super::{Checked, Result, fail, package};
use local_agent_core::plugin::external::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    path::Path,
};
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

mod bindings {
    wasmtime::component::bindgen!({path:"../../contracts/plugins",world:"plugin"});
}
use bindings::Plugin;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RpcRequest {
    pub jsonrpc: String,
    pub id: String,
    pub method: String,
    pub params: Value,
}
pub(crate) fn read_line(reader: &mut impl BufRead) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        let input = reader.fill_buf().checked("worker_crashed")?;
        if input.is_empty() {
            return Err(fail("worker_crashed"));
        }
        let end = input.iter().position(|b| *b == b'\n');
        let count = end.map_or(input.len(), |n| n + 1);
        if bytes.len() + count > MAX_FRAME {
            return Err(fail("resource_limit"));
        }
        bytes.extend_from_slice(&input[..count]);
        reader.consume(count);
        if end.is_some() {
            break;
        }
    }
    Ok(bytes)
}
pub(crate) fn send(value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(value).checked("invalid_input")?;
    bytes.push(b'\n');
    if bytes.len() > MAX_FRAME {
        return Err(fail("resource_limit"));
    }
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&bytes).checked("worker_crashed")?;
    stdout.flush().checked("worker_crashed")
}
struct GuestState {
    violation: Option<&'static str>,
    limits: StoreLimits,
    context: Option<Initialize>,
    phase: String,
    counter: u64,
}
impl bindings::local_agent::plugin::host::Host for GuestState {
    fn call(&mut self, method: String, payload: String) -> std::result::Result<String, String> {
        let response = (|| -> Result<String> {
            if payload.len() > MAX_CHUNK
                || !matches!(
                    (self.phase.as_str(), method.as_str()),
                    (
                        "session.scan"
                            | "session.read"
                            | "domain.scan"
                            | "domain.read-conversation",
                        "source.read"
                    ) | ("account.quota", "connection.request")
                )
            {
                return Err(fail("permission_denied"));
            }
            let context = self.context.as_ref().ok_or(fail("permission_denied"))?;
            self.counter += 1;
            if self.counter > 128 {
                return Err(fail("resource_limit"));
            }
            let id = format!("runner-{}", self.counter);
            send(
                &json!({"jsonrpc":"2.0","id":id,"method":"host.call","params":{"contextId":context.context_id,"method":method,"payload":payload}}),
            )?;
            let bytes = read_line(&mut std::io::stdin().lock())?;
            let value: Value = serde_json::from_slice(&bytes).checked("invalid_input")?;
            if value["jsonrpc"] != "2.0" || value["id"] != id {
                return Err(fail("invalid_input"));
            }
            if value.get("error").is_some() {
                return Err(fail("permission_denied"));
            }
            let result = value["result"].as_str().ok_or(fail("invalid_input"))?;
            if result.len() > MAX_FRAME / 2 {
                return Err(fail("resource_limit"));
            }
            Ok(result.to_owned())
        })();
        response.map_err(|e| {
            self.violation = Some(e.code);
            e.code.to_owned()
        })
    }
}
pub(crate) fn run(path: &Path, digest: &str) -> Result<()> {
    run_package(path, digest)
}
struct WorkerPackage {
    long_lived: bool,
    capabilities: Vec<String>,
    settings: Vec<Setting>,
    module: Vec<u8>,
}
fn load_package(bytes: &[u8], digest: &str) -> Result<WorkerPackage> {
    let mut checked = package::inspect(
        bytes,
        std::env::args().nth(4).as_deref() == Some("development"),
    )?;
    if checked.digest != digest {
        return Err(fail("signature_invalid"));
    }
    let Runtime::WasmComponent { entry, .. } = &checked.manifest.runtime else {
        return Err(fail("unsupported"));
    };
    Ok(WorkerPackage {
        long_lived: checked.manifest.is_extension(),
        capabilities: checked.manifest.capabilities,
        settings: checked.manifest.settings,
        module: checked.files.remove(entry).ok_or(fail("invalid_input"))?,
    })
}
fn run_package(path: &Path, digest: &str) -> Result<()> {
    // This private branch runs before service/vault/runtime initialization and receives
    // an empty environment. Package inputs are verified again inside the worker.
    // CoreFoundation inserts this locale marker on macOS even with env_clear.
    // No WASI/environment import is linked, so the guest cannot access it.
    if std::env::vars_os().any(|(key, _)| key != "__CF_USER_TEXT_ENCODING") {
        return Err(fail("permission_denied"));
    }
    let bytes = package::read_file(path, package::MAX_ARCHIVE)?;
    // Development status is supplied by the trusted parent via a separate fixed arg;
    // signatures, when present, are always verified even in that mode.
    let package = load_package(&bytes, digest)?;
    let cpu = libc::rlimit {
        rlim_cur: 10,
        rlim_max: 11,
    };
    // Process CPU ceiling complements guest fuel and parent wall-clock cancellation.
    if unsafe { libc::setrlimit(libc::RLIMIT_CPU, &cpu) } != 0 {
        return Err(fail("resource_limit"));
    }
    let mut config = Config::new();
    config
        .consume_fuel(true)
        .epoch_interruption(true)
        .max_wasm_stack(512 * 1024)
        .wasm_component_model(true)
        .wasm_memory64(false)
        .wasm_multi_memory(false);
    let engine = Engine::new(&config).checked("worker_crashed")?;
    let component = Component::new(&engine, &package.module).checked("invalid_input")?;
    let limits = StoreLimitsBuilder::new()
        .memory_size(128 * 1_048_576)
        .instances(8)
        .memories(4)
        .tables(8)
        .table_elements(10_000)
        .trap_on_grow_failure(true)
        .build();
    let mut store = Store::new(
        &engine,
        GuestState {
            violation: None,
            limits,
            context: None,
            phase: "created".into(),
            counter: 0,
        },
    );
    store.limiter(|s| &mut s.limits);
    store.set_fuel(100_000_000).checked("resource_limit")?;
    store.set_epoch_deadline(1);
    let clock = engine.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(if package.long_lived {
            300
        } else {
            30
        }));
        clock.increment_epoch();
    });
    let mut linker = Linker::new(&engine);
    Plugin::add_to_linker::<_, wasmtime::component::HasSelf<GuestState>>(
        &mut linker,
        |s: &mut GuestState| s,
    )
    .checked("invalid_input")?;
    let plugin = Plugin::instantiate(&mut store, &component, &linker).checked("invalid_input")?;
    loop {
        let request: RpcRequest = serde_json::from_slice(&read_line(&mut std::io::stdin().lock())?)
            .checked("invalid_input")?;
        if request.jsonrpc != "2.0" || !request.id.starts_with("host-") || request.id.len() > 100 {
            return Err(fail("invalid_input"));
        }
        let phase = store.data().phase.as_str();
        let allowed = match request.method.as_str() {
            "plugin.initialize" => phase == "created",
            "plugin.prepare" => phase == "initialized",
            "plugin.activate" => phase == "prepared",
            "plugin.health" => phase == "active",
            "session.scan" | "session.read" => {
                phase == "active" && package.capabilities.iter().any(|c| c == "session.source@1")
            }
            "account.quota" => {
                phase == "active"
                    && package
                        .capabilities
                        .iter()
                        .any(|c| c == "account.quota-source@1")
            }
            "domain.scan" | "domain.read-conversation" => {
                phase == "active"
                    && package
                        .capabilities
                        .iter()
                        .any(|c| c == "domain.session-source@1")
            }
            "policy.rank" => {
                phase == "active"
                    && package.capabilities == [local_agent_core::plugin::policy::CAPABILITY]
            }
            "protocol.open" | "protocol.feed" | "protocol.finish" => {
                phase == "active"
                    && package
                        .capabilities
                        .iter()
                        .any(|c| c == "gateway.protocol@1")
            }
            "config.propose" => {
                phase == "active"
                    && package
                        .capabilities
                        .iter()
                        .any(|c| c == "config.plan-provider@1")
            }
            "provider.catalog.request" | "provider.catalog.parse" => {
                phase == "active"
                    && package
                        .capabilities
                        .iter()
                        .any(|c| c == local_agent_core::domain::provider_access::CAPABILITY)
            }
            "plugin.deactivate" => phase == "active",
            "plugin.dispose" => phase == "disabled",
            _ => false,
        };
        if !allowed {
            send(
                &json!({"jsonrpc":"2.0","id":request.id,"error":{"code":-32601,"message":"unsupported","data":{"code":"unsupported"}}}),
            )?;
            continue;
        }
        if request.method == "plugin.initialize" {
            let init: Initialize =
                serde_json::from_value(request.params.clone()).checked("invalid_input")?;
            if init.api_version != API_VERSION
                || init.package_digest != digest
                || init.capabilities != package.capabilities
                || init.fuel != "100000000"
                || init.max_memory_bytes != "134217728"
                || uuid::Uuid::parse_str(&init.context_id).is_err()
            {
                return Err(fail("incompatible_api"));
            }
            if init.settings.len() != package.settings.len()
                || package.settings.iter().any(|s| {
                    init.settings
                        .get(&s.key)
                        .is_none_or(|v| !s.options.contains(v))
                })
            {
                return Err(fail("invalid_input"));
            }
            store.data_mut().context = Some(init);
        }
        let previous = store.data().phase.clone();
        store.data_mut().phase = request.method.clone();
        let payload = serde_json::to_string(&request.params).checked("invalid_input")?;
        let result = plugin.call_invoke(&mut store, &request.method, &payload);
        let value = if let Some(code) = store.data().violation {
            Err(fail(code))
        } else {
            match result {
                Ok(Ok(value)) if value.len() <= MAX_FRAME / 2 => {
                    serde_json::from_str::<Value>(&value).checked("invalid_input")
                }
                Ok(Ok(_)) => Err(fail("resource_limit")),
                Ok(Err(_)) => Err(fail("invalid_input")),
                Err(_) => Err(fail("resource_limit")),
            }
        };
        match value {
            Ok(value) => {
                if request.method == "plugin.initialize" {
                    let echoed: Initialize =
                        serde_json::from_value(value.clone()).checked("incompatible_api")?;
                    if serde_json::to_value(echoed).checked("invalid_input")? != request.params {
                        return Err(fail("incompatible_api"));
                    }
                } else if request.method.starts_with("plugin.") && value != json!({"ready":true}) {
                    return Err(fail("invalid_input"));
                }
                store.data_mut().phase = match request.method.as_str() {
                    "plugin.initialize" => "initialized",
                    "plugin.prepare" => "prepared",
                    "plugin.activate" => "active",
                    "plugin.deactivate" => "disabled",
                    _ => &previous,
                }
                .into();
                send(&json!({"jsonrpc":"2.0","id":request.id,"result":value}))?;
                if request.method == "plugin.dispose" {
                    return Ok(());
                }
            }
            Err(error) => {
                send(
                    &json!({"jsonrpc":"2.0","id":request.id,"error":{"code":-32000,"message":error.code,"data":{"code":error.code}}}),
                )?;
                return Err(error);
            }
        }
    }
}
