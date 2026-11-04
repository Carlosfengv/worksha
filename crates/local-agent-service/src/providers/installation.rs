use super::process::{cli_version, output};
use local_agent_core::providers::{ConfigError, config};
use semver::Version;
use std::{
    ffi::OsStr,
    fs::{self, File},
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
};

const MAC_BIN_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"];
const CODEX_BINARIES: &[&str] = &[
    "Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
    "Contents/Resources/codex-cli/bin/codex",
    "Contents/Resources/codex",
];

struct Locations {
    path: Vec<PathBuf>,
    applications: Vec<PathBuf>,
}

impl Locations {
    fn current() -> Self {
        let nvm = std::env::var_os("NVM_DIR").map(PathBuf::from);
        Self::new(
            dirs::home_dir().as_deref(),
            &std::env::var_os("PATH").unwrap_or_default(),
            Path::new("/Applications"),
            MAC_BIN_DIRS.iter().map(PathBuf::from),
            nvm.as_deref().filter(|path| path.is_absolute()),
        )
    }

    fn new(
        home: Option<&Path>,
        inherited: &OsStr,
        system_apps: &Path,
        system_bin: impl IntoIterator<Item = PathBuf>,
        nvm: Option<&Path>,
    ) -> Self {
        let mut path: Vec<_> = std::env::split_paths(inherited)
            .filter(|dir| dir.is_absolute())
            .collect();
        let mut applications = Vec::new();
        if let Some(home) = home {
            path.extend(
                [
                    ".local/bin",
                    ".opencode/bin",
                    ".bun/bin",
                    ".npm/bin",
                    ".npm-global/bin",
                    ".volta/bin",
                    ".asdf/shims",
                    ".local/share/mise/shims",
                    ".local/share/pnpm",
                    "Library/pnpm",
                    ".claude/local",
                    ".codex/packages/standalone/current/bin",
                ]
                .map(|dir| home.join(dir)),
            );
            applications.push(home.join("Applications"));
        }
        let mut node_bins = Vec::new();
        if let Some(home) = home {
            let default_nvm = home.join(".nvm");
            let nvm = nvm.unwrap_or(&default_nvm);
            path.push(nvm.join("current/bin"));
            node_bins = fs::read_dir(nvm.join("versions/node"))
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let name = entry.file_name();
                    let version = Version::parse(name.to_str()?.strip_prefix('v')?).ok()?;
                    Some((version, entry.path().join("bin")))
                })
                .collect();
            node_bins.sort_unstable_by(|a, b| b.0.cmp(&a.0));
            if let Some(default) = nvm_default(nvm, &node_bins) {
                path.push(default);
            }
        }
        path.extend(system_bin);
        path.extend(node_bins.into_iter().map(|(_, bin)| bin));
        path.extend(["/usr/bin", "/bin"].map(PathBuf::from));
        applications.push(system_apps.to_owned());
        Self { path, applications }
    }

    fn cli(&self, agent: &str) -> Option<PathBuf> {
        self.path
            .iter()
            .map(|dir| dir.join(agent))
            .find(|path| executable(path))
    }

    fn app(&self, name: &str) -> impl Iterator<Item = PathBuf> + '_ {
        let name = format!("{name}.app");
        self.applications
            .iter()
            .flat_map(move |dir| {
                let name = name.clone();
                let direct = dir.join(&name);
                let nested = fs::read_dir(dir)
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                    .filter(move |entry| {
                        let filename = entry.file_name();
                        !filename.to_string_lossy().starts_with('.')
                            && Path::new(&filename).extension() != Some(OsStr::new("app"))
                            && entry.file_type().is_ok_and(|kind| kind.is_dir())
                    })
                    .map(move |entry| entry.path().join(&name));
                std::iter::once(direct).chain(nested)
            })
            .filter(|app| app.is_dir())
    }

    fn bundled_codex(&self) -> Option<PathBuf> {
        self.app("Codex")
            .chain(self.app("ChatGPT"))
            .find_map(|app| {
                CODEX_BINARIES
                    .iter()
                    .map(|relative| app.join(relative))
                    .find(|path| executable(path))
            })
    }

    fn probe_cli(&self, path: &Path) -> Option<Version> {
        let search_path = std::env::join_paths(
            path.parent()
                .into_iter()
                .chain(self.path.iter().map(PathBuf::as_path)),
        )
        .ok()?;
        cli_version(path, &search_path).as_deref().and_then(parse)
    }
}

fn nvm_default(nvm: &Path, versions: &[(Version, PathBuf)]) -> Option<PathBuf> {
    let mut alias = String::from("default");
    // Resolve bounded local aliases, including lts/*, without loading shell code.
    for _ in 0..8 {
        if !Path::new(&alias)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        {
            return None;
        }
        let mut value = String::new();
        if File::open(nvm.join("alias").join(&alias))
            .and_then(|file| file.take(256).read_to_string(&mut value))
            .is_err()
        {
            break;
        }
        alias = value
            .lines()
            .find(|line| !line.trim().is_empty())?
            .trim()
            .into();
    }
    let selector = alias.strip_prefix('v').unwrap_or(&alias);
    versions
        .iter()
        .find(|(version, bin)| {
            let current = version.to_string();
            executable(&bin.join("node"))
                && (matches!(selector, "node" | "stable")
                    || current == selector
                    || current
                        .strip_prefix(selector)
                        .is_some_and(|rest| rest.starts_with('.')))
        })
        .map(|(_, bin)| bin.clone())
}

fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

pub(super) fn binary(agent: &str) -> Option<PathBuf> {
    let locations = Locations::current();
    locations.cli(agent).or_else(|| {
        (agent == "codex")
            .then(|| locations.bundled_codex())
            .flatten()
    })
}

fn parse(text: &str) -> Option<Version> {
    text.split_whitespace()
        .filter(|word| word.len() <= 80)
        .find_map(|word| Version::parse(word.strip_prefix('v').unwrap_or(word)).ok())
}

fn bundle_version(app: &Path) -> Option<Version> {
    output(
        Path::new("/usr/libexec/PlistBuddy"),
        &[
            "-c",
            "Print CFBundleShortVersionString",
            app.join("Contents/Info.plist").to_str()?,
        ],
    )
    .as_deref()
    .and_then(parse)
}

pub(super) struct Compatibility {
    pub(super) version: Option<String>,
    pub(super) result: Result<(), ConfigError>,
}

pub(super) fn compatibility(agent: &str) -> Compatibility {
    detect(agent, &Locations::current())
}

fn detect(agent: &str, locations: &Locations) -> Compatibility {
    let installations: Vec<_> = match agent {
        "claude" => locations
            .cli(agent)
            .map(|path| ("Claude Code CLI", locations.probe_cli(&path)))
            .into_iter()
            .collect(),
        "claude-desktop" => locations
            .app("Claude")
            .next()
            .map(|app| ("Claude 桌面端", bundle_version(&app)))
            .into_iter()
            .collect(),
        "opencode" => locations
            .cli(agent)
            .map(|path| ("OpenCode CLI", locations.probe_cli(&path)))
            .into_iter()
            .chain(
                locations
                    .app("OpenCode")
                    .next()
                    .map(|app| ("OpenCode 桌面端", bundle_version(&app))),
            )
            .collect(),
        "codex" => {
            let cli = locations.cli(agent);
            let bundled = locations.bundled_codex().filter(|path| {
                cli.as_deref() != Some(path)
                    && cli
                        .as_deref()
                        .and_then(|cli| fs::canonicalize(cli).ok())
                        .as_ref()
                        != Some(path)
            });
            cli.map(|path| ("Codex CLI", locations.probe_cli(&path)))
                .into_iter()
                .chain(bundled.map(|path| ("Codex 桌面端", locations.probe_cli(&path))))
                .collect()
        }
        _ => Vec::new(),
    };
    assess(agent, installations)
}

fn assess(agent: &str, installations: Vec<(&str, Option<Version>)>) -> Compatibility {
    let version = installations
        .iter()
        .find_map(|(_, version)| version.as_ref())
        .map(ToString::to_string);
    let result = (|| {
        let name = match agent {
            "claude" => "Claude Code CLI",
            "claude-desktop" => "Claude 桌面端",
            "codex" => "Codex",
            "opencode" => "OpenCode",
            _ => return Err(ConfigError::new("unsupported", "不支持此 Agent")),
        };
        let adapter = config::builtin().resolve(agent)?;
        if installations.is_empty() {
            return Err(ConfigError::new(
                "agent_not_found",
                &format!("未找到 {name}。请确认已安装后重试。"),
            ));
        }
        for (name, detected) in installations {
            let detected = detected.ok_or_else(|| {
                ConfigError::new(
                    "agent_version_unreadable",
                    &format!("已找到 {name}，但无法读取版本。请确认客户端可正常启动后重试。"),
                )
            })?;
            if !adapter.version_supported(&detected.to_string()) {
                return Err(ConfigError::new(
                    "agent_version_unverified",
                    &format!("{name} {detected} 的配置适配尚未验证。请使用已验证的版本后重试。"),
                ));
            }
        }
        Ok(())
    })();
    Compatibility { version, result }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str, mode: u32) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn finder_path_finds_package_manager_installs_and_supplies_their_interpreter() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let homebrew = home.join("homebrew/bin");
        let inherited = home.join("inherited");
        write(&inherited.join("claude"), "not executable", 0o600);
        write(
            &homebrew.join("claude"),
            "#!/usr/bin/env fixture-node\n",
            0o700,
        );
        write(
            &homebrew.join("fixture-node"),
            "#!/bin/sh\necho '2.1.318 (Claude Code)'\n",
            0o700,
        );
        let nvm = home.join(".nvm/versions/node");
        write(
            &nvm.join("v24.9.0/bin/opencode"),
            "#!/bin/sh\necho '1.18.13'\n",
            0o700,
        );
        write(
            &nvm.join("v24.10.0/bin/opencode"),
            "#!/bin/sh\necho 'v1.19.5'\n",
            0o700,
        );

        let locations = Locations::new(
            Some(home),
            inherited.as_os_str(),
            &home.join("system"),
            [homebrew],
            None,
        );
        for (agent, expected) in [("claude", "2.1.318"), ("opencode", "1.19.5")] {
            let outcome = detect(agent, &locations);
            assert_eq!(outcome.version.as_deref(), Some(expected));
            outcome.result.unwrap();
        }
        write(&home.join(".nvm/alias/default"), "lts/*\n", 0o600);
        write(&home.join(".nvm/alias/lts/*"), "v24.9\n", 0o600);
        write(&nvm.join("v24.9.0/bin/node"), "#!/bin/sh\n", 0o700);
        let locations = Locations::new(Some(home), OsStr::new(""), &home.join("system"), [], None);
        let preferred = detect("opencode", &locations);
        assert_eq!(preferred.version.as_deref(), Some("1.18.13"));
        preferred.result.unwrap();
    }

    #[test]
    fn desktop_and_cli_sharing_configuration_are_checked() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let bundled = home
            .join("Applications/AI/Codex.app")
            .join(CODEX_BINARIES[2]);
        write(
            &bundled,
            "#!/bin/sh\necho 'codex-cli 0.166.0-alpha.5.1'\n",
            0o700,
        );
        let locations = Locations::new(Some(home), OsStr::new(""), &home.join("system"), [], None);
        assert_eq!(locations.bundled_codex(), Some(bundled.clone()));
        detect("codex", &locations).result.unwrap();
        write(
            &home.join(".local/bin/codex"),
            "#!/bin/sh\necho 'codex-cli 0.166.0'\n",
            0o700,
        );
        write(&bundled, "#!/bin/sh\necho 'codex-cli 0.167.0'\n", 0o700);
        let error = detect("codex", &locations).result.unwrap_err();
        assert_eq!(error.code, "agent_version_unverified");
        assert!(error.message.contains("Codex 桌面端 0.167.0"));
    }

    #[test]
    fn absent_failed_and_unverified_cli_probes_have_distinct_errors() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let locations = Locations::new(Some(home), OsStr::new(""), &home.join("system"), [], None);
        let cli = home.join(".local/bin/claude");
        assert_eq!(
            detect("claude", &locations).result.unwrap_err().code,
            "agent_not_found"
        );
        write(&cli, "#!/bin/sh\necho 2.1.318\nexit 2\n", 0o700);
        assert_eq!(
            detect("claude", &locations).result.unwrap_err().code,
            "agent_version_unreadable"
        );
        write(&cli, "#!/bin/sh\necho '2.1.319 (Claude Code)'\n", 0o700);
        let error = detect("claude", &locations).result.unwrap_err();
        assert_eq!(error.code, "agent_version_unverified");
        assert!(error.message.contains("2.1.319"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_desktop_versions_without_launching_the_application() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let locations = Locations::new(Some(home), OsStr::new(""), &home.join("system"), [], None);
        for (app, agent, version) in [
            ("Claude", "claude-desktop", "3.6556.2"),
            ("OpenCode", "opencode", "1.19.5"),
        ] {
            let plist = home.join(format!("Applications/{app}.app/Contents/Info.plist"));
            write(
                &plist,
                &format!(
                    r#"<plist version="1.0"><dict><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>"#
                ),
                0o600,
            );
            let detected = detect(agent, &locations);
            assert_eq!(detected.version.as_deref(), Some(version));
            detected.result.unwrap();
        }
    }
}
