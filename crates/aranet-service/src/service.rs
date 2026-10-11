//! Service management for aranet-service.
//!
//! Installs and controls aranet-service with the platform's service manager,
//! through the service-manager crate.
//!
//! A user-level service (`--user`) runs as the user who installed it. A
//! system-level service is installed only on systemd, as the hardened
//! `aranet.service`: the dynamic user `aranet` runs it, with its config and
//! database in `/var/lib/aranet`. Every other service manager would run a
//! system-level service as root (or LocalSystem), so those installs are refused.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Component, Path, PathBuf};

use service_manager::{
    RestartPolicy, ServiceInstallCtx, ServiceLabel, ServiceLevel, ServiceManager,
    ServiceManagerKind, ServiceStartCtx, ServiceStatusCtx, ServiceStopCtx, ServiceUninstallCtx,
};
use thiserror::Error;

/// Service label for launchd (`dev.rye.aranet.plist`), sc, WinSW, OpenRC and rc.d.
const SERVICE_LABEL: &str = "dev.rye.aranet";

/// Service label for systemd, which service-manager writes as `aranet.service`.
const SYSTEMD_LABEL: &str = "aranet";

/// The label 0.2.1 and earlier also used on systemd. service-manager names a
/// systemd unit `<organisation>-<application>`, so it was `rye-aranet.service`.
const LEGACY_SYSTEMD_LABEL: &str = "dev.rye.aranet";

/// The system unit's state directory (`StateDirectory=aranet`), the only place
/// the service can write.
const SYSTEMD_STATE_DIR: &str = "/var/lib/aranet";

/// The system unit's config file. It lives in the state directory because the
/// service replaces it (a temporary file renamed over it) when devices are
/// added, changed or removed through the API.
const SYSTEMD_CONFIG_PATH: &str = "/var/lib/aranet/server.toml";

/// Where the refusals suggest copying the binary for a system service.
const RECOMMENDED_PROGRAM: &str = "/usr/local/bin/aranet-service";

/// Directories the system unit's sandbox hides from the service, and why.
const HIDDEN_FROM_SERVICE: [(&str, &str); 5] = [
    ("/home", "ProtectHome=yes hides /home from the service"),
    ("/root", "ProtectHome=yes hides /root from the service"),
    (
        "/run/user",
        "ProtectHome=yes hides /run/user from the service",
    ),
    ("/tmp", "PrivateTmp=yes gives the service its own /tmp"),
    (
        "/var/tmp",
        "PrivateTmp=yes gives the service its own /var/tmp",
    ),
];

/// The LaunchDaemon that `sudo aranet-service service install` wrote on macOS
/// up to 0.2.1. launchd runs it as root.
const LEGACY_LAUNCH_DAEMON: &str = "/Library/LaunchDaemons/dev.rye.aranet.plist";

/// Where a system service installed by 0.2.1 or earlier, which ran as root,
/// kept its config.
const LEGACY_ROOT_CONFIG: &str = "/root/.config/aranet/server.toml";

/// Errors that can occur during service management.
#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("No service manager available on this platform")]
    NoServiceManager,

    #[error("Service manager error: {0}")]
    Manager(#[from] std::io::Error),

    #[error("Could not find aranet-service executable")]
    ExecutableNotFound,

    #[error("User-level services not supported on this platform")]
    UserLevelNotSupported,

    #[error("Failed to resolve current working directory: {0}")]
    CurrentDirectory(#[source] std::io::Error),

    #[error(
        "{path} can be modified by a non-root user ({reason}), so a system service must not \
         run it. {}",
        executable_remedy(.path)
    )]
    InsecureExecutable { path: PathBuf, reason: String },

    #[error(
        "{path} can't be run by the system service's unprivileged user ({reason}). {}",
        executable_remedy(.path)
    )]
    UnrunnableExecutable { path: PathBuf, reason: String },

    #[error("System-level services are not supported with {manager}: {hint}")]
    SystemLevelUnsupported {
        manager: &'static str,
        hint: &'static str,
    },

    #[error("{path} can't be used by the system service: {reason}")]
    InaccessiblePath { path: PathBuf, reason: &'static str },

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
}

/// Service management level
#[derive(Debug, Clone, Copy, Default)]
pub enum Level {
    /// System-level service (requires root/admin)
    #[default]
    System,
    /// User-level service (no elevated privileges needed)
    User,
}

/// The kind of service manager this platform uses.
fn native_kind() -> Result<ServiceManagerKind, ServiceError> {
    ServiceManagerKind::native().map_err(|_| ServiceError::NoServiceManager)
}

/// Get the service manager of `kind`, set to `level`.
fn get_manager(
    kind: ServiceManagerKind,
    level: Level,
) -> Result<Box<dyn ServiceManager>, ServiceError> {
    let mut manager = <dyn ServiceManager>::target(kind);

    let service_level = match level {
        Level::System => ServiceLevel::System,
        Level::User => ServiceLevel::User,
    };

    manager
        .set_level(service_level)
        .map_err(|_| ServiceError::UserLevelNotSupported)?;

    Ok(manager)
}

/// Get the path to the aranet-service executable for a user-level service.
///
/// Prefers well-known install locations over the current executable path
/// to ensure the service uses a stable binary location:
/// - macOS: /usr/local/bin/aranet-service or ~/Library/bin/aranet-service
/// - Linux: /usr/local/bin/aranet-service or ~/.local/bin/aranet-service
/// - Windows: Program Files paths
///
/// Falls back to current_exe() if no installed binary is found.
fn get_executable_path() -> Result<PathBuf, ServiceError> {
    // Check well-known install locations
    let candidates = get_install_candidates();

    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    // Fall back to the current executable (may be a debug build)
    env::current_exe().map_err(|_| ServiceError::ExecutableNotFound)
}

/// The executable a system-level service runs: the one that was started, with
/// symlinks resolved so that the checks see the real file. Nothing else is probed.
fn system_program() -> Result<PathBuf, ServiceError> {
    let exe = env::current_exe().map_err(|_| ServiceError::ExecutableNotFound)?;
    fs::canonicalize(exe).map_err(|_| ServiceError::ExecutableNotFound)
}

/// Get candidate paths where aranet-service might be installed.
fn get_install_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    #[cfg(target_os = "macos")]
    {
        // Homebrew or manual install
        candidates.push(PathBuf::from("/usr/local/bin/aranet-service"));
        candidates.push(PathBuf::from("/opt/homebrew/bin/aranet-service"));

        // User-local installs
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".cargo/bin/aranet-service"));
            candidates.push(home.join("Library/bin/aranet-service"));
        }
    }

    #[cfg(target_os = "linux")]
    {
        candidates.push(PathBuf::from("/usr/local/bin/aranet-service"));
        candidates.push(PathBuf::from("/usr/bin/aranet-service"));

        // User-local installs
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".cargo/bin/aranet-service"));
            candidates.push(home.join(".local/bin/aranet-service"));
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Program Files
        if let Ok(program_files) = env::var("ProgramFiles") {
            candidates
                .push(PathBuf::from(&program_files).join("aranet-service/aranet-service.exe"));
        }

        // User-local installs
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".cargo/bin/aranet-service.exe"));
        }
    }

    candidates
}

/// The service label for `kind`: `aranet` (`aranet.service`) on systemd,
/// `dev.rye.aranet` everywhere else.
fn label(kind: ServiceManagerKind) -> ServiceLabel {
    match kind {
        ServiceManagerKind::Systemd => parse_label(SYSTEMD_LABEL),
        ServiceManagerKind::Launchd
        | ServiceManagerKind::OpenRc
        | ServiceManagerKind::Rcd
        | ServiceManagerKind::Sc
        | ServiceManagerKind::WinSw => parse_label(SERVICE_LABEL),
    }
}

/// The systemd label of 0.2.1 and earlier (`rye-aranet.service`).
fn legacy_systemd_label() -> ServiceLabel {
    parse_label(LEGACY_SYSTEMD_LABEL)
}

fn parse_label(name: &str) -> ServiceLabel {
    name.parse().expect("Invalid service label")
}

/// The unit file service-manager writes for `label` in `unit_dir`.
fn unit_file(unit_dir: &Path, label: &ServiceLabel) -> PathBuf {
    unit_dir.join(format!("{}.service", label.to_script_name()))
}

/// The directory service-manager writes systemd units to at `level`.
fn systemd_unit_dir(level: Level) -> Result<PathBuf, ServiceError> {
    match level {
        Level::System => Ok(service_manager::systemd_global_dir_path()),
        Level::User => Ok(service_manager::systemd_user_dir_path()?),
    }
}

/// Install aranet-service as a service.
///
/// A user-level service runs the installed binary with `options`, as before.
/// A system-level service is installed only on systemd, and only if no
/// non-root user can modify the binary that was run and the service's
/// unprivileged user can run it. The config the service will read is
/// validated first. On systemd, a `rye-aranet.service` left by 0.2.1 or
/// earlier at the same level is stopped and removed.
pub fn install(level: Level, options: &aranet_service::RunOptions) -> Result<(), ServiceError> {
    let kind = native_kind()?;
    let program = match level {
        Level::System => system_program()?,
        Level::User => get_executable_path()?,
    };
    let ctx = install_ctx(kind, level, program, options, writable_by_non_root)?;
    if matches!(level, Level::System)
        && let Some(reason) = not_runnable_by_service(&ctx.program)
    {
        return Err(ServiceError::UnrunnableExecutable {
            path: ctx.program,
            reason,
        });
    }

    // Validate the config the service will read, without writing anything:
    // `sudo` must never create a root-owned config.
    let mut checked = options.clone();
    if kind == ServiceManagerKind::Systemd && matches!(level, Level::System) {
        checked.config = Some(systemd_config_path(options)?);
    }
    aranet_service::check_config(&checked)
        .map_err(|e| ServiceError::InvalidConfig(format!("{e:#}")))?;

    let manager = get_manager(kind, level)?;
    let unit_dir = match kind {
        ServiceManagerKind::Systemd => Some(systemd_unit_dir(level)?),
        _ => None,
    };
    install_with(manager.as_ref(), unit_dir.as_deref(), ctx)
}

/// Remove a legacy systemd unit in `unit_dir` (when given), then install `ctx`.
fn install_with(
    manager: &dyn ServiceManager,
    unit_dir: Option<&Path>,
    ctx: ServiceInstallCtx,
) -> Result<(), ServiceError> {
    if let Some(unit_dir) = unit_dir {
        remove_legacy_unit(manager, unit_dir)?;
    }
    manager.install(ctx)?;
    Ok(())
}

/// The service-manager install context for `kind` at `level`.
///
/// User level: service-manager's own template, running `program` with the
/// options, as in earlier versions.
///
/// System level: refused unless `kind` is systemd, the only manager that can
/// run the service unprivileged without any setup. On systemd,
/// `program_check` must find no way for a non-root user to modify `program`,
/// and the unit is the hardened `aranet.service` from `render_systemd_unit`.
fn install_ctx(
    kind: ServiceManagerKind,
    level: Level,
    program: PathBuf,
    options: &aranet_service::RunOptions,
    program_check: impl Fn(&Path) -> Option<String>,
) -> Result<ServiceInstallCtx, ServiceError> {
    let (args, contents) = match level {
        Level::User => (build_install_args(options)?, None),
        Level::System => {
            if let Some(refusal) = system_level_refusal(kind) {
                return Err(refusal);
            }
            if let Some(reason) = program_check(&program) {
                return Err(ServiceError::InsecureExecutable {
                    path: program,
                    reason,
                });
            }
            let args = systemd_system_args(options)?;
            let unit = render_systemd_unit(&systemd_exec_start(&program, &args)?);
            (args, Some(unit))
        }
    };

    Ok(ServiceInstallCtx {
        label: label(kind),
        program,
        args,
        contents,
        username: None,
        working_directory: None,
        environment: None,
        autostart: true,
        restart_policy: RestartPolicy::OnFailure {
            delay_secs: Some(5),
        },
    })
}

/// Why a system-level service can't be installed with `kind`; `None` for systemd.
fn system_level_refusal(kind: ServiceManagerKind) -> Option<ServiceError> {
    let (manager, hint) = match kind {
        ServiceManagerKind::Systemd => return None,
        ServiceManagerKind::Launchd => (
            "launchd",
            "use `aranet-service service install --user` instead: it installs a LaunchAgent that \
             runs as you, while a LaunchDaemon would run as root and can't get Bluetooth \
             permission",
        ),
        ServiceManagerKind::Sc | ServiceManagerKind::WinSw => (
            "the Windows service manager",
            "a Windows service would run as LocalSystem, and aranet-service can't run as one; \
             start `aranet-service run` at logon with Task Scheduler instead (see the docs)",
        ),
        ServiceManagerKind::OpenRc => (
            "OpenRC",
            "it would run aranet-service as root; write your own service file that runs \
             `aranet-service run` as an unprivileged user",
        ),
        ServiceManagerKind::Rcd => (
            "rc.d",
            "it would run aranet-service as root; write your own rc.d script that runs \
             `aranet-service run` as an unprivileged user",
        ),
    };
    Some(ServiceError::SystemLevelUnsupported { manager, hint })
}

/// Why a non-root user could modify or replace `path`, or `None` if only root can.
///
/// `path` must be absolute, and it and every directory above it must be owned
/// by root and writable neither by other users nor by a group other than
/// root's. `stat` returns `(uid, gid, mode)`; an error counts as writable.
fn writable_by_non_root_with(
    path: &Path,
    stat: impl Fn(&Path) -> std::io::Result<(u32, u32, u32)>,
) -> Option<String> {
    if !path.is_absolute() {
        return Some(format!("{} is not an absolute path", path.display()));
    }
    for entry in path.ancestors() {
        let (uid, gid, mode) = match stat(entry) {
            Ok(owner_and_mode) => owner_and_mode,
            Err(e) => return Some(format!("can't check {}: {e}", entry.display())),
        };
        if uid != 0 {
            return Some(format!("{} is owned by uid {uid}", entry.display()));
        }
        if mode & 0o002 != 0 {
            return Some(format!("{} is writable by every user", entry.display()));
        }
        if mode & 0o020 != 0 && gid != 0 {
            return Some(format!("{} is writable by group {gid}", entry.display()));
        }
    }
    None
}

/// `writable_by_non_root_with` on the real file system.
fn writable_by_non_root(path: &Path) -> Option<String> {
    writable_by_non_root_with(path, owner_and_mode)
}

/// Why the system service's unprivileged user couldn't run `path`, or `None`
/// if it can.
///
/// The unit's sandbox must not hide `path`, and because the program is owned
/// by root, it must be executable, and every directory above it searchable,
/// by other users. `stat` returns `(uid, gid, mode)`; an error counts as
/// unrunnable.
fn not_runnable_by_service_with(
    path: &Path,
    stat: impl Fn(&Path) -> std::io::Result<(u32, u32, u32)>,
) -> Option<String> {
    if let Some(reason) = hidden_from_service(path) {
        return Some(reason.to_string());
    }
    for entry in path.ancestors() {
        let (_, _, mode) = match stat(entry) {
            Ok(owner_and_mode) => owner_and_mode,
            Err(e) => return Some(format!("can't check {}: {e}", entry.display())),
        };
        if mode & 0o001 == 0 {
            let access = if entry == path {
                "executable"
            } else {
                "searchable"
            };
            return Some(format!(
                "{} is not {access} by other users",
                entry.display()
            ));
        }
    }
    None
}

/// `not_runnable_by_service_with` on the real file system.
fn not_runnable_by_service(path: &Path) -> Option<String> {
    not_runnable_by_service_with(path, owner_and_mode)
}

/// `(uid, gid, mode)` of `entry`, following symlinks.
#[cfg(unix)]
fn owner_and_mode(entry: &Path) -> std::io::Result<(u32, u32, u32)> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::metadata(entry)?;
    Ok((metadata.uid(), metadata.gid(), metadata.mode()))
}

/// Without Unix ownership every program counts as unsafe. System-level
/// installs are refused on Windows before the checks that call this.
#[cfg(not(unix))]
fn owner_and_mode(_entry: &Path) -> std::io::Result<(u32, u32, u32)> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "ownership checks are only implemented on Unix",
    ))
}

/// The end of the `InsecureExecutable` and `UnrunnableExecutable` messages: how
/// to give the system service a copy that it can run and only root can change.
fn executable_remedy(program: &Path) -> String {
    if program == Path::new(RECOMMENDED_PROGRAM) {
        // Copying it onto itself can't help: its modes, or a directory's above it, are the problem.
        format!(
            "Make it and every directory above it owned by root and writable only by root, and \
             executable by every user (check `ls -ld / /usr /usr/local /usr/local/bin \
             {RECOMMENDED_PROGRAM}`)"
        )
    } else {
        format!(
            "Copy it first: `sudo install -o root -g root -m 0755 {} {RECOMMENDED_PROGRAM}`, then \
             run `sudo {RECOMMENDED_PROGRAM} service install`",
            program.display()
        )
    }
}

/// Why the system unit's sandbox would hide `path` from the service, or why
/// that can't be ruled out. `None` if `path` is in none of the hidden
/// directories; whether the service may read the file is not checked here.
///
/// `path` is compared with each `HIDDEN_FROM_SERVICE` directory twice, by
/// whole components:
///
/// - As written. The service can't pass through a hidden directory, wherever
///   a symlink inside it leads.
/// - With symlinks resolved, because the sandbox hides the directories
///   themselves, whatever name leads to them. Of `path`, the nearest ancestor
///   that exists is resolved and the components below it are kept as written:
///   a config file need not exist yet. A symlink whose target doesn't exist
///   counts as not existing, so it is not followed. A hidden directory that
///   exists is resolved (on some systems `/home` is a symlink to `/var/home`);
///   one that doesn't is compared as written.
///
/// A `path` with a `..` component is refused without comparing it: where
/// `..` leads depends on symlinks in directories that may not exist yet.
fn hidden_from_service(path: &Path) -> Option<&'static str> {
    hidden_under(path, &HIDDEN_FROM_SERVICE)
}

/// `hidden_from_service` for the directories in `roots`, each with its reason.
fn hidden_under<P: AsRef<Path>>(path: &Path, roots: &[(P, &'static str)]) -> Option<&'static str> {
    if path.components().any(|c| c == Component::ParentDir) {
        return Some(
            "a path with `..` can't be checked against the directories the unit hides; give it \
             without `..`",
        );
    }
    let resolved = resolve_existing_part(path);
    roots
        .iter()
        .find(|(root, _)| {
            let root = root.as_ref();
            path.starts_with(root)
                || resolved.starts_with(fs::canonicalize(root).unwrap_or_else(|_| root.into()))
        })
        .map(|&(_, reason)| reason)
}

/// `path` with its symlinks resolved as far as it exists: its nearest ancestor
/// that exists, canonicalized, then the components below that one as written.
fn resolve_existing_part(path: &Path) -> PathBuf {
    for ancestor in path.ancestors() {
        if let (Ok(mut resolved), Ok(below)) =
            (fs::canonicalize(ancestor), path.strip_prefix(ancestor))
        {
            resolved.extend(below);
            return resolved;
        }
    }
    path.to_path_buf()
}

/// The config file of the systemd system service: `--config`, made absolute,
/// or `/var/lib/aranet/server.toml`.
fn systemd_config_path(options: &aranet_service::RunOptions) -> Result<PathBuf, ServiceError> {
    match &options.config {
        Some(config) => resolve_service_path(config),
        None => Ok(PathBuf::from(SYSTEMD_CONFIG_PATH)),
    }
}

/// The arguments of the system unit's `ExecStart=`: `run --config <file>` and
/// the given overrides.
///
/// The unit hides home directories, gives the service its own `/tmp` and lets
/// it write only `/var/lib/aranet`, so a config or database it couldn't use
/// is refused here instead of failing when the service starts. The config is
/// refused if `hidden_from_service` finds it in a hidden directory, as written
/// or with symlinks resolved as far as the path exists, or if it has a `..`
/// component. The database is compared as written with `/var/lib/aranet` and
/// must not have a `..` component either.
fn systemd_system_args(
    options: &aranet_service::RunOptions,
) -> Result<Vec<OsString>, ServiceError> {
    let config = systemd_config_path(options)?;
    if let Some(reason) = hidden_from_service(&config) {
        return Err(ServiceError::InaccessiblePath {
            path: config,
            reason,
        });
    }

    let mut args = vec![
        OsString::from("run"),
        OsString::from("--config"),
        config.into_os_string(),
    ];

    if let Some(bind) = &options.bind {
        args.push(OsString::from("--bind"));
        args.push(OsString::from(bind));
    }

    if let Some(database) = &options.database {
        let database = resolve_service_path(database)?;
        if !is_within(&database, Path::new(SYSTEMD_STATE_DIR)) {
            return Err(ServiceError::InaccessiblePath {
                path: database,
                reason: "the service can only write /var/lib/aranet",
            });
        }
        args.push(OsString::from("--database"));
        args.push(database.into_os_string());
    }

    if options.no_collector {
        args.push(OsString::from("--no-collector"));
    }

    Ok(args)
}

/// Whether `path` is `dir` or below it, without `..` components.
fn is_within(path: &Path, dir: &Path) -> bool {
    path.starts_with(dir) && !path.components().any(|c| c == Component::ParentDir)
}

/// The `ExecStart=` value that runs `program` with `args`.
///
/// systemd expands `%` specifiers on the whole line, splits it into words that
/// it unquotes and unescapes C-style, and substitutes `$` variables in the
/// arguments. So `%` is doubled everywhere and `$` in the arguments, and a word
/// that is empty, is a lone `;`, or contains whitespace, a quote or a backslash
/// is put in double quotes with `\` and `"` escaped. (service-manager's own
/// template joins the words unquoted.) systemd refuses an executable path with
/// a quote or a backslash, and a unit file can't carry non-UTF-8 text or
/// control characters, so those are refused here.
fn systemd_exec_start(program: &Path, args: &[OsString]) -> Result<String, ServiceError> {
    let program_text = unit_text(program.as_os_str())?;
    if program_text.contains(['"', '\'', '\\']) {
        return Err(ServiceError::InaccessiblePath {
            path: program.to_path_buf(),
            reason: "systemd refuses executable paths with quotes or backslashes",
        });
    }

    let mut words = vec![quote_unit_word(program_text.replace('%', "%%"))];
    for arg in args {
        let escaped = unit_text(arg)?.replace('%', "%%").replace('$', "$$");
        words.push(quote_unit_word(escaped));
    }
    Ok(words.join(" "))
}

/// `word` as text a unit file can hold: UTF-8 without control characters.
fn unit_text(word: &OsStr) -> Result<&str, ServiceError> {
    let unusable = |reason: &'static str| ServiceError::InaccessiblePath {
        path: PathBuf::from(word),
        reason,
    };
    let text = word
        .to_str()
        .ok_or_else(|| unusable("systemd unit files must be UTF-8"))?;
    if text.chars().any(char::is_control) {
        return Err(unusable(
            "systemd unit files can't contain control characters",
        ));
    }
    Ok(text)
}

/// `word` as one `ExecStart=` word, double-quoted if systemd would otherwise
/// split, unescape or drop it.
fn quote_unit_word(word: String) -> String {
    let needs_quotes = word.is_empty()
        || word == ";"
        || word
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\'));
    if !needs_quotes {
        return word;
    }
    format!("\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The system unit, exactly as shipped in `distribution/systemd/aranet.service`
/// (a test keeps the two identical), with `exec_start` as its `ExecStart=`.
///
/// It runs as the dynamic user `aranet` (or a static `aranet` account if one
/// exists) with no capabilities: btleplug reaches BlueZ over D-Bus. The config
/// and the database live in the state directory, the only writable path;
/// `ARANET_DATA_DIR` makes `/var/lib/aranet/data.db` the default database, and
/// `UMask=0077` keeps both owner-only. `AF_NETLINK` lets mDNS list the network
/// interfaces. Where the BlueZ D-Bus policy admits only the `bluetooth` group,
/// a drop-in adds `SupplementaryGroups=bluetooth`; the unit can't, because
/// systemd refuses to start it on systems without that group.
fn render_systemd_unit(exec_start: &str) -> String {
    format!(
        "\
[Unit]
Description=Aranet sensor collector and HTTP API
Documentation=https://github.com/cameronrye/aranet
After=network-online.target bluetooth.target
Wants=network-online.target bluetooth.target
StartLimitIntervalSec=60
StartLimitBurst=3

[Service]
Type=simple
ExecStart={exec_start}
Restart=on-failure
RestartSec=5
Environment=ARANET_DATA_DIR=/var/lib/aranet
EnvironmentFile=-/etc/default/aranet
User=aranet
Group=aranet
DynamicUser=yes
StateDirectory=aranet
StateDirectoryMode=0750
UMask=0077
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
CapabilityBoundingSet=
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK

[Install]
WantedBy=multi-user.target
"
    )
}

/// Stop and remove the `rye-aranet.service` that 0.2.1 and earlier installed,
/// if `unit_dir` has one. Returns whether there was one.
fn remove_legacy_unit(manager: &dyn ServiceManager, unit_dir: &Path) -> std::io::Result<bool> {
    let legacy = legacy_systemd_label();
    if !unit_file(unit_dir, &legacy).try_exists()? {
        return Ok(false);
    }
    // Stopping fails when the unit isn't loaded; removing it is what matters.
    let _ = manager.stop(ServiceStopCtx {
        label: legacy.clone(),
    });
    manager.uninstall(ServiceUninstallCtx { label: legacy })?;
    Ok(true)
}

/// What to tell the user while services that 0.2.1 or earlier installed are
/// still there, one line per service, or `None` if there are none.
///
/// On systemd that is the root `rye-aranet.service` at either level: `service
/// start`, `stop` and `status` no longer address it, it runs as root, and it
/// holds the port that a new `aranet.service`, system or user, needs. `--user`
/// commands also report a user `rye-aranet.service`. On macOS it is the root
/// LaunchDaemon, at either level for the same reasons.
pub fn legacy_service_warning(level: Level) -> Option<String> {
    let warnings: Vec<&str> = legacy_services(native_kind().ok()?, level)
        .into_iter()
        .filter(|(file, _)| file.exists())
        .map(|(_, warning)| warning)
        .collect();
    (!warnings.is_empty()).then(|| warnings.join("\n"))
}

/// The files that services from 0.2.1 or earlier left for `kind`, as a command
/// at `level` reports them, each with its one-line warning.
fn legacy_services(kind: ServiceManagerKind, level: Level) -> Vec<(PathBuf, &'static str)> {
    let mut services = Vec::new();
    match kind {
        ServiceManagerKind::Systemd => {
            let legacy = legacy_systemd_label();
            if matches!(level, Level::User)
                && let Ok(user_dir) = service_manager::systemd_user_dir_path()
            {
                services.push((
                    unit_file(&user_dir, &legacy),
                    "rye-aranet.service, the user service that aranet-service 0.2.1 or earlier \
                     installed, is still installed; `service start --user`, `stop --user` and \
                     `status --user` now manage aranet.service. Replace it with \
                     `aranet-service service install --user`, or remove it with \
                     `aranet-service service uninstall --user`",
                ));
            }
            services.push((
                unit_file(&service_manager::systemd_global_dir_path(), &legacy),
                "rye-aranet.service, the system service that aranet-service 0.2.1 or earlier \
                 installed, is still installed and runs as root; `service start`, `stop` and \
                 `status` now manage aranet.service. Replace it with \
                 `sudo aranet-service service install`, or remove it with \
                 `sudo aranet-service service uninstall`",
            ));
        }
        ServiceManagerKind::Launchd => services.push((
            PathBuf::from(LEGACY_LAUNCH_DAEMON),
            "/Library/LaunchDaemons/dev.rye.aranet.plist, which `sudo aranet-service service \
             install` wrote up to 0.2.1, runs aranet-service as root. Remove it with \
             `sudo aranet-service service uninstall`, then use \
             `aranet-service service install --user`",
        )),
        // 0.2.1 could install root services here too. They aren't detected, but
        // `sudo aranet-service service uninstall` still removes them: the label
        // is unchanged.
        ServiceManagerKind::OpenRc
        | ServiceManagerKind::Rcd
        | ServiceManagerKind::Sc
        | ServiceManagerKind::WinSw => {}
    }
    services
}

/// Printed after a system-level install, which only succeeds on systemd.
const SYSTEM_INSTALL_NOTES: &[&str] = &[
    "aranet.service runs as the unprivileged dynamic user `aranet`. Its config and database",
    "are in /var/lib/aranet, which systemd creates when the service first starts.",
    "  Start it:     sudo systemctl start aranet",
    "  Add sensors:  sudoedit /var/lib/aranet/server.toml, then sudo systemctl restart aranet",
    "  Logs:         journalctl -u aranet",
];

/// Printed after `SYSTEM_INSTALL_NOTES` when a root service from 0.2.1 or
/// earlier left its config in /root. The globs run in `sh -c` because the
/// invoking user's shell can't list /root or /var/lib/aranet.
const MOVE_ROOT_FILES_NOTES: &[&str] = &[
    "",
    "Earlier versions ran the service as root with its files under /root. To keep them:",
    "  1. sudo systemctl start aranet && sudo systemctl stop aranet  (creates /var/lib/aranet)",
    "  2. sudo cp /root/.config/aranet/server.toml /var/lib/aranet/",
    "  3. sudo sh -c 'rm -f /var/lib/aranet/data.db*'",
    "  4. sudo sh -c 'cp /root/.local/share/aranet/data.db* /var/lib/aranet/'",
    "  5. sudo chown -R --reference=/var/lib/aranet /var/lib/aranet/",
    "  6. If /var/lib/aranet/server.toml sets storage.path under /root, delete that line",
    "  7. sudo systemctl start aranet",
];

/// What to print after a system-level install.
pub fn system_install_notes() -> String {
    render_system_install_notes(Path::new(LEGACY_ROOT_CONFIG).exists())
}

fn render_system_install_notes(legacy_root_config: bool) -> String {
    let mut lines = SYSTEM_INSTALL_NOTES.to_vec();
    if legacy_root_config {
        lines.extend_from_slice(MOVE_ROOT_FILES_NOTES);
    }
    let mut notes = lines.join("\n");
    notes.push('\n');
    notes
}

fn build_install_args(options: &aranet_service::RunOptions) -> Result<Vec<OsString>, ServiceError> {
    let mut args = vec![OsString::from("run")];

    if let Some(config) = &options.config {
        args.push(OsString::from("--config"));
        args.push(resolve_service_path(config)?.into_os_string());
    }

    if let Some(bind) = &options.bind {
        args.push(OsString::from("--bind"));
        args.push(OsString::from(bind));
    }

    if let Some(database) = &options.database {
        args.push(OsString::from("--database"));
        args.push(resolve_service_path(database)?.into_os_string());
    }

    if options.no_collector {
        args.push(OsString::from("--no-collector"));
    }

    Ok(args)
}

fn resolve_service_path(path: &Path) -> Result<PathBuf, ServiceError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()
            .map_err(ServiceError::CurrentDirectory)?
            .join(path))
    }
}

/// Uninstall the aranet-service system service.
///
/// On systemd this also stops and removes a `rye-aranet.service` that 0.2.1 or
/// earlier installed at the same level.
pub fn uninstall(level: Level) -> Result<(), ServiceError> {
    let kind = native_kind()?;
    let manager = get_manager(kind, level)?;
    let unit_dir = match kind {
        ServiceManagerKind::Systemd => Some(systemd_unit_dir(level)?),
        _ => None,
    };
    uninstall_with(manager.as_ref(), kind, unit_dir.as_deref())
}

/// Remove a legacy systemd unit in `unit_dir` (when given), then the current
/// service. If only the legacy unit was installed, removing it is enough.
fn uninstall_with(
    manager: &dyn ServiceManager,
    kind: ServiceManagerKind,
    unit_dir: Option<&Path>,
) -> Result<(), ServiceError> {
    let current = label(kind);
    if let Some(unit_dir) = unit_dir
        && remove_legacy_unit(manager, unit_dir)?
        && !unit_file(unit_dir, &current).try_exists()?
    {
        return Ok(());
    }
    manager.uninstall(ServiceUninstallCtx { label: current })?;
    Ok(())
}

/// Start the aranet-service system service.
pub fn start(level: Level) -> Result<(), ServiceError> {
    let kind = native_kind()?;
    let manager = get_manager(kind, level)?;

    manager
        .start(ServiceStartCtx { label: label(kind) })
        .map_err(ServiceError::Manager)?;

    Ok(())
}

/// Stop the aranet-service system service.
pub fn stop(level: Level) -> Result<(), ServiceError> {
    let kind = native_kind()?;
    let manager = get_manager(kind, level)?;

    manager
        .stop(ServiceStopCtx { label: label(kind) })
        .map_err(ServiceError::Manager)?;

    Ok(())
}

/// Get the status of the aranet-service.
///
/// This checks if the service is running by querying the service manager
/// or falling back to checking if the HTTP API is reachable.
pub fn status(level: Level) -> Result<ServiceStatus, ServiceError> {
    let kind = native_kind()?;
    let manager = get_manager(kind, level)?;

    Ok(status_with(
        manager.as_ref(),
        label(kind),
        is_service_reachable,
    ))
}

/// The status `manager` reports for `label`. If it can't tell, a reachable
/// HTTP API (`reachable`) counts as running.
fn status_with(
    manager: &dyn ServiceManager,
    label: ServiceLabel,
    reachable: impl FnOnce() -> bool,
) -> ServiceStatus {
    match manager.status(ServiceStatusCtx { label }) {
        Ok(status) => match status {
            service_manager::ServiceStatus::Running => ServiceStatus::Running,
            service_manager::ServiceStatus::Stopped(_) => ServiceStatus::Stopped,
            service_manager::ServiceStatus::NotInstalled => ServiceStatus::NotInstalled,
        },
        Err(_) => {
            // Fall back to checking if the API is reachable
            if reachable() {
                ServiceStatus::Running
            } else {
                ServiceStatus::Stopped
            }
        }
    }
}

/// Check if the service API is reachable by attempting a TCP connection.
///
/// Uses the configured bind address from the service config, falling back
/// to the default address if config cannot be loaded.
fn is_service_reachable() -> bool {
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    let bind = aranet_service::Config::load_default()
        .map(|c| c.server.bind)
        .unwrap_or_else(|_| "127.0.0.1:8080".to_string());

    let addr: SocketAddr = match bind.parse() {
        Ok(addr) => addr,
        Err(_) => return false,
    };

    TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_ok()
}

/// Service status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceStatus {
    Running,
    Stopped,
    NotInstalled,
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;

    const PROGRAM: &str = "/usr/local/bin/aranet-service";

    fn panicking_program_check(_: &Path) -> Option<String> {
        panic!("only a system-level systemd install checks the program")
    }

    /// Records every call it gets as `"<method> <label>"`, and reports `status`
    /// (an error when `None`). With `stop_fails`, `stop` records its call and
    /// then fails.
    #[derive(Default)]
    struct FakeManager {
        calls: RefCell<Vec<String>>,
        status: Option<service_manager::ServiceStatus>,
        stop_fails: bool,
    }

    impl FakeManager {
        fn record(&self, method: &str, label: &ServiceLabel) {
            self.calls.borrow_mut().push(format!("{method} {label}"));
        }

        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl ServiceManager for FakeManager {
        fn available(&self) -> std::io::Result<bool> {
            Ok(true)
        }

        fn install(&self, ctx: ServiceInstallCtx) -> std::io::Result<()> {
            self.record("install", &ctx.label);
            Ok(())
        }

        fn uninstall(&self, ctx: ServiceUninstallCtx) -> std::io::Result<()> {
            self.record("uninstall", &ctx.label);
            Ok(())
        }

        fn start(&self, ctx: ServiceStartCtx) -> std::io::Result<()> {
            self.record("start", &ctx.label);
            Ok(())
        }

        fn stop(&self, ctx: ServiceStopCtx) -> std::io::Result<()> {
            self.record("stop", &ctx.label);
            if self.stop_fails {
                return Err(std::io::Error::other("systemctl stop failed"));
            }
            Ok(())
        }

        fn level(&self) -> ServiceLevel {
            ServiceLevel::System
        }

        fn set_level(&mut self, _level: ServiceLevel) -> std::io::Result<()> {
            Ok(())
        }

        fn status(&self, ctx: ServiceStatusCtx) -> std::io::Result<service_manager::ServiceStatus> {
            self.record("status", &ctx.label);
            self.status
                .clone()
                .ok_or_else(|| std::io::Error::other("systemctl status failed"))
        }
    }

    /// A unit directory holding empty unit files with these names.
    fn unit_dir_with(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for file in files {
            fs::write(dir.path().join(file), "[Service]\n").unwrap();
        }
        dir
    }

    /// The context of a system-level systemd install of `PROGRAM` with default options.
    fn systemd_system_ctx() -> ServiceInstallCtx {
        install_ctx(
            ServiceManagerKind::Systemd,
            Level::System,
            PathBuf::from(PROGRAM),
            &aranet_service::RunOptions::default(),
            |_| None,
        )
        .unwrap()
    }

    #[test]
    fn test_build_install_args_preserves_runtime_options() {
        let db_path = env::temp_dir().join("aranet-test.db");
        let options = aranet_service::RunOptions {
            config: Some(PathBuf::from("config/server.toml")),
            bind: Some("0.0.0.0:9090".to_string()),
            database: Some(db_path.clone()),
            no_collector: true,
        };

        let args = build_install_args(&options).unwrap();
        let cwd = env::current_dir().unwrap();

        assert_eq!(
            args,
            vec![
                OsString::from("run"),
                OsString::from("--config"),
                cwd.join("config/server.toml").into_os_string(),
                OsString::from("--bind"),
                OsString::from("0.0.0.0:9090"),
                OsString::from("--database"),
                db_path.into_os_string(),
                OsString::from("--no-collector"),
            ]
        );
    }

    #[test]
    fn test_build_install_args_omits_unset_options() {
        let args = build_install_args(&aranet_service::RunOptions::default()).unwrap();

        assert_eq!(args, vec![OsString::from("run")]);
    }

    #[test]
    fn system_install_on_systemd_uses_hardened_unit() {
        let ctx = systemd_system_ctx();

        assert_eq!(ctx.label.to_script_name(), "aranet");
        assert_eq!(ctx.program, PathBuf::from(PROGRAM));
        assert_eq!(ctx.username, None);
        assert!(ctx.autostart);
        let unit = ctx
            .contents
            .expect("a system-level systemd install writes its own unit");
        let lines: Vec<&str> = unit.lines().collect();
        for expected in [
            "ExecStart=/usr/local/bin/aranet-service run --config /var/lib/aranet/server.toml",
            "User=aranet",
            "DynamicUser=yes",
            "StateDirectory=aranet",
            "Environment=ARANET_DATA_DIR=/var/lib/aranet",
            "ProtectHome=yes",
            "CapabilityBoundingSet=",
        ] {
            assert!(
                lines.contains(&expected),
                "missing {expected:?} in:\n{unit}"
            );
        }
        assert!(!unit.contains("AmbientCapabilities"), "{unit}");
    }

    #[test]
    fn unit_matches_distribution_file() {
        // Read at run time: no crate compiles in a file from distribution/,
        // which the Docker build context and the published crate leave out.
        // A Windows checkout may have converted the file to CRLF.
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        if !repo.join("crates/aranet-service/Cargo.toml").is_file() {
            return; // not a repository checkout: the published crate has no distribution/
        }
        let shipped = fs::read_to_string(repo.join("distribution/systemd/aranet.service"))
            .expect("read distribution/systemd/aranet.service")
            .replace("\r\n", "\n");

        assert_eq!(
            render_systemd_unit(
                "/usr/local/bin/aranet-service run --config /var/lib/aranet/server.toml"
            ),
            shipped,
            "distribution/systemd/aranet.service must be the unit that `service install` writes"
        );
    }

    #[test]
    fn system_install_refused_on_non_systemd() {
        for kind in [
            ServiceManagerKind::Launchd,
            ServiceManagerKind::Sc,
            ServiceManagerKind::WinSw,
            ServiceManagerKind::OpenRc,
            ServiceManagerKind::Rcd,
        ] {
            let result = install_ctx(
                kind,
                Level::System,
                PathBuf::from(PROGRAM),
                &aranet_service::RunOptions::default(),
                panicking_program_check,
            );
            assert!(
                matches!(result, Err(ServiceError::SystemLevelUnsupported { .. })),
                "{kind:?}: {result:?}"
            );
        }

        let launchd = install_ctx(
            ServiceManagerKind::Launchd,
            Level::System,
            PathBuf::from(PROGRAM),
            &aranet_service::RunOptions::default(),
            panicking_program_check,
        )
        .unwrap_err()
        .to_string();
        assert!(
            launchd.contains("aranet-service service install --user"),
            "{launchd}"
        );
    }

    #[test]
    fn user_install_keeps_template() {
        let options = aranet_service::RunOptions {
            bind: Some("127.0.0.1:9090".to_string()),
            no_collector: true,
            ..aranet_service::RunOptions::default()
        };
        let program = PathBuf::from("/opt/aranet/aranet-service");

        for (kind, expected_label) in [
            (ServiceManagerKind::Systemd, "aranet"),
            (ServiceManagerKind::Launchd, "dev.rye.aranet"),
        ] {
            let ctx = install_ctx(
                kind,
                Level::User,
                program.clone(),
                &options,
                panicking_program_check,
            )
            .unwrap();
            assert_eq!(ctx.label.to_qualified_name(), expected_label, "{kind:?}");
            assert_eq!(ctx.program, program, "{kind:?}");
            assert_eq!(ctx.args, build_install_args(&options).unwrap(), "{kind:?}");
            assert_eq!(ctx.contents, None, "{kind:?}");
            assert_eq!(ctx.username, None, "{kind:?}");
        }
    }

    #[test]
    fn insecure_program_is_refused_with_copy_hint() {
        let program = PathBuf::from("/opt/aranet/aranet-service");
        let err = install_ctx(
            ServiceManagerKind::Systemd,
            Level::System,
            program.clone(),
            &aranet_service::RunOptions::default(),
            |_| Some("/opt/aranet is owned by uid 1000".to_string()),
        )
        .unwrap_err();
        assert!(
            matches!(&err, ServiceError::InsecureExecutable { path, .. } if *path == program),
            "{err:?}"
        );
        let message = err.to_string();
        assert!(
            message.contains("(/opt/aranet is owned by uid 1000)"),
            "{message}"
        );
        assert!(
            message.contains(
                "sudo install -o root -g root -m 0755 /opt/aranet/aranet-service \
                 /usr/local/bin/aranet-service"
            ),
            "{message}"
        );

        // Already in /usr/local/bin: no copy onto itself.
        let message = install_ctx(
            ServiceManagerKind::Systemd,
            Level::System,
            PathBuf::from(PROGRAM),
            &aranet_service::RunOptions::default(),
            |_| Some("/usr/local/bin is writable by every user".to_string()),
        )
        .unwrap_err()
        .to_string();
        assert!(!message.contains("sudo install"), "{message}");
        assert!(
            message.contains("ls -ld / /usr /usr/local /usr/local/bin"),
            "{message}"
        );
    }

    #[test]
    fn exec_start_quotes_and_escapes() {
        let exec_start = |program: &str, args: &[&str]| {
            let args: Vec<OsString> = args.iter().map(OsString::from).collect();
            systemd_exec_start(Path::new(program), &args)
        };

        assert_eq!(
            exec_start("/opt/aranet tools/aranet-service", &["run"]).unwrap(),
            r#""/opt/aranet tools/aranet-service" run"#
        );
        // systemd expands `%` in the whole line but `$` only in the arguments.
        assert_eq!(
            exec_start("/opt/50%/$x/aranet-service", &["--bind", "50%"]).unwrap(),
            "/opt/50%%/$x/aranet-service --bind 50%%"
        );
        assert_eq!(
            exec_start(PROGRAM, &["--config", "$HOME/a b.toml"]).unwrap(),
            r#"/usr/local/bin/aranet-service --config "$$HOME/a b.toml""#
        );
        assert_eq!(
            exec_start(PROGRAM, &[r#"a"b\c"#, "it's", "", ";"]).unwrap(),
            r#"/usr/local/bin/aranet-service "a\"b\\c" "it's" "" ";""#
        );
        assert!(matches!(
            exec_start("/opt/it's/aranet-service", &["run"]),
            Err(ServiceError::InaccessiblePath { .. })
        ));
        assert!(matches!(
            exec_start(PROGRAM, &["--bind", "127.0.0.1:8080\nExecStartPre=/bin/sh"]),
            Err(ServiceError::InaccessiblePath { .. })
        ));
    }

    #[test]
    fn install_removes_legacy_unit() {
        let dir = unit_dir_with(&["rye-aranet.service"]);
        let manager = FakeManager::default();
        install_with(&manager, Some(dir.path()), systemd_system_ctx()).unwrap();
        assert_eq!(
            manager.calls(),
            [
                "stop dev.rye.aranet",
                "uninstall dev.rye.aranet",
                "install aranet"
            ]
        );

        let dir = unit_dir_with(&["aranet.service"]);
        let manager = FakeManager::default();
        install_with(&manager, Some(dir.path()), systemd_system_ctx()).unwrap();
        assert_eq!(manager.calls(), ["install aranet"]);
    }

    #[test]
    fn uninstall_removes_both_units() {
        let cases: [(&[&str], &[&str]); 3] = [
            (
                &["rye-aranet.service", "aranet.service"],
                &[
                    "stop dev.rye.aranet",
                    "uninstall dev.rye.aranet",
                    "uninstall aranet",
                ],
            ),
            (
                &["rye-aranet.service"],
                &["stop dev.rye.aranet", "uninstall dev.rye.aranet"],
            ),
            // service-manager reports the missing aranet.service.
            (&[], &["uninstall aranet"]),
        ];
        for (files, expected) in cases {
            let dir = unit_dir_with(files);
            let manager = FakeManager::default();
            uninstall_with(&manager, ServiceManagerKind::Systemd, Some(dir.path())).unwrap();
            assert_eq!(manager.calls(), expected, "{files:?}");
        }

        let manager = FakeManager::default();
        uninstall_with(&manager, ServiceManagerKind::Launchd, None).unwrap();
        assert_eq!(manager.calls(), ["uninstall dev.rye.aranet"]);
    }

    #[test]
    fn legacy_services_table() {
        let root_unit = Path::new("/etc/systemd/system/rye-aranet.service");

        let system = legacy_services(ServiceManagerKind::Systemd, Level::System);
        assert_eq!(system.len(), 1, "{system:?}");
        let (file, warning) = &system[0];
        assert_eq!(file, root_unit);
        assert!(warning.contains("runs as root"), "{warning}");
        assert!(
            warning.contains("sudo aranet-service service install"),
            "{warning}"
        );

        // `--user` commands, which the GUI runs, report the root unit too: it
        // runs as root and holds the port a user-level aranet.service needs.
        let user = legacy_services(ServiceManagerKind::Systemd, Level::User);
        assert_eq!(user.len(), 2, "{user:?}");
        let (file, warning) = &user[0];
        assert_eq!(file.file_name().unwrap(), "rye-aranet.service");
        assert_ne!(file, root_unit);
        assert!(
            warning.contains("aranet-service service uninstall --user"),
            "{warning}"
        );
        assert_eq!(user[1], system[0]);

        for level in [Level::System, Level::User] {
            let launchd = legacy_services(ServiceManagerKind::Launchd, level);
            assert_eq!(launchd.len(), 1, "{launchd:?}");
            assert_eq!(
                launchd[0].0,
                Path::new("/Library/LaunchDaemons/dev.rye.aranet.plist")
            );
        }
        for kind in [
            ServiceManagerKind::OpenRc,
            ServiceManagerKind::Rcd,
            ServiceManagerKind::Sc,
            ServiceManagerKind::WinSw,
        ] {
            assert!(legacy_services(kind, Level::System).is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn system_install_notes_table() {
        let fresh = render_system_install_notes(false);
        for expected in [
            "dynamic user `aranet`",
            "sudo systemctl start aranet",
            "sudoedit /var/lib/aranet/server.toml",
            "journalctl -u aranet",
        ] {
            assert!(
                fresh.contains(expected),
                "missing {expected:?} in:\n{fresh}"
            );
        }
        assert!(!fresh.contains("/root"), "{fresh}");

        let upgrade = render_system_install_notes(true);
        assert!(upgrade.starts_with(&fresh), "{upgrade}");
        // The seven steps end the notes, word for word and in this order.
        // Documentation that repeats them has to match.
        let steps = [
            "  1. sudo systemctl start aranet && sudo systemctl stop aranet  (creates /var/lib/aranet)",
            "  2. sudo cp /root/.config/aranet/server.toml /var/lib/aranet/",
            "  3. sudo sh -c 'rm -f /var/lib/aranet/data.db*'",
            "  4. sudo sh -c 'cp /root/.local/share/aranet/data.db* /var/lib/aranet/'",
            "  5. sudo chown -R --reference=/var/lib/aranet /var/lib/aranet/",
            "  6. If /var/lib/aranet/server.toml sets storage.path under /root, delete that line",
            "  7. sudo systemctl start aranet",
        ];
        let lines: Vec<&str> = upgrade.lines().collect();
        assert!(
            lines.ends_with(&steps),
            "the upgrade notes must end with these steps:\n{steps:#?}\nbut they are:\n{upgrade}"
        );
    }

    #[test]
    fn status_reports_not_installed() {
        let cases = [
            (
                service_manager::ServiceStatus::NotInstalled,
                ServiceStatus::NotInstalled,
            ),
            (
                service_manager::ServiceStatus::Running,
                ServiceStatus::Running,
            ),
            (
                service_manager::ServiceStatus::Stopped(None),
                ServiceStatus::Stopped,
            ),
        ];
        for (reported, expected) in cases {
            let manager = FakeManager {
                status: Some(reported.clone()),
                ..FakeManager::default()
            };
            let status = status_with(&manager, label(ServiceManagerKind::Systemd), || {
                panic!("the API is only probed when the service manager fails")
            });
            assert_eq!(status, expected, "{reported:?}");
            assert_eq!(manager.calls(), ["status aranet"]);
        }

        // The service manager failed: fall back to the API.
        let manager = FakeManager::default();
        let systemd = label(ServiceManagerKind::Systemd);
        assert_eq!(
            status_with(&manager, systemd.clone(), || true),
            ServiceStatus::Running
        );
        assert_eq!(
            status_with(&manager, systemd, || false),
            ServiceStatus::Stopped
        );
    }

    #[test]
    fn refusals_and_the_launch_daemon_warning_say_what_to_do_instead() {
        let cases = [
            (
                ServiceManagerKind::Launchd,
                "launchd",
                "use `aranet-service service install --user` instead",
            ),
            (
                ServiceManagerKind::Sc,
                "the Windows service manager",
                "start `aranet-service run` at logon with Task Scheduler instead",
            ),
            (
                ServiceManagerKind::WinSw,
                "the Windows service manager",
                "start `aranet-service run` at logon with Task Scheduler instead",
            ),
            (
                ServiceManagerKind::OpenRc,
                "OpenRC",
                "write your own service file that runs `aranet-service run` as an unprivileged user",
            ),
            (
                ServiceManagerKind::Rcd,
                "rc.d",
                "write your own rc.d script that runs `aranet-service run` as an unprivileged user",
            ),
        ];
        for (kind, manager, advice) in cases {
            let message = install_ctx(
                kind,
                Level::System,
                PathBuf::from(PROGRAM),
                &aranet_service::RunOptions::default(),
                panicking_program_check,
            )
            .unwrap_err()
            .to_string();
            assert!(
                message.starts_with(&format!(
                    "System-level services are not supported with {manager}: "
                )),
                "{kind:?}: {message}"
            );
            assert!(message.contains(advice), "{kind:?}: {message}");
        }

        // A program the service's user couldn't run gets the same remedy as
        // one that a non-root user could replace.
        let hidden = ServiceError::UnrunnableExecutable {
            path: PathBuf::from("/root/.cargo/bin/aranet-service"),
            reason: "ProtectHome=yes hides /root from the service".to_string(),
        }
        .to_string();
        assert!(
            hidden.contains("(ProtectHome=yes hides /root from the service)"),
            "{hidden}"
        );
        assert!(
            hidden.contains(
                "sudo install -o root -g root -m 0755 /root/.cargo/bin/aranet-service \
                 /usr/local/bin/aranet-service"
            ),
            "{hidden}"
        );
        let in_place = ServiceError::UnrunnableExecutable {
            path: PathBuf::from(PROGRAM),
            reason: "/usr/local/bin is not searchable by other users".to_string(),
        }
        .to_string();
        assert!(!in_place.contains("sudo install"), "{in_place}");
        assert!(
            in_place.contains("ls -ld / /usr /usr/local /usr/local/bin"),
            "{in_place}"
        );

        // The LaunchDaemon can only be removed, never replaced: a system-level
        // install is refused on macOS.
        for level in [Level::System, Level::User] {
            let launchd = legacy_services(ServiceManagerKind::Launchd, level);
            let (_, warning) = &launchd[0];
            assert!(warning.contains("runs aranet-service as root"), "{warning}");
            assert!(
                warning.contains(
                    "Remove it with `sudo aranet-service service uninstall`, then use \
                     `aranet-service service install --user`"
                ),
                "{warning}"
            );
        }
    }

    #[test]
    fn exec_start_quotes_each_special_character_on_its_own() {
        let exec_start = |program: &str, args: &[&str]| {
            let args: Vec<OsString> = args.iter().map(OsString::from).collect();
            systemd_exec_start(Path::new(program), &args)
        };

        // Each of these alone makes systemd split or unescape an unquoted word.
        for (arg, word) in [
            (r#"a"b"#, r#""a\"b""#),
            (r"a\b", r#""a\\b""#),
            ("a'b", r#""a'b""#),
            ("a b", r#""a b""#),
        ] {
            assert_eq!(
                exec_start(PROGRAM, &[arg]).unwrap(),
                format!("{PROGRAM} {word}"),
                "{arg}"
            );
        }

        // systemd refuses each of these in an executable path.
        for program in [
            r#"/opt/a"b/aranet-service"#,
            r"/opt/a\b/aranet-service",
            "/opt/a'b/aranet-service",
        ] {
            match exec_start(program, &["run"]) {
                Err(ServiceError::InaccessiblePath { path, reason }) => {
                    assert_eq!(path, Path::new(program));
                    assert_eq!(
                        reason,
                        "systemd refuses executable paths with quotes or backslashes"
                    );
                }
                other => panic!("{program}: {other:?}"),
            }
        }

        // An argument that isn't UTF-8 is refused, not written as something else.
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;

            let args = [
                OsString::from("--config"),
                OsString::from_vec(b"/etc/aranet/\xff.toml".to_vec()),
            ];
            match systemd_exec_start(Path::new(PROGRAM), &args) {
                Err(ServiceError::InaccessiblePath { reason, .. }) => {
                    assert_eq!(reason, "systemd unit files must be UTF-8");
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn legacy_unit_is_removed_even_if_stopping_it_fails() {
        // `systemctl stop` fails for a unit file that systemd hasn't loaded.
        let dir = unit_dir_with(&["rye-aranet.service"]);
        let manager = FakeManager {
            stop_fails: true,
            ..FakeManager::default()
        };
        install_with(&manager, Some(dir.path()), systemd_system_ctx()).unwrap();
        assert_eq!(
            manager.calls(),
            [
                "stop dev.rye.aranet",
                "uninstall dev.rye.aranet",
                "install aranet"
            ]
        );

        let dir = unit_dir_with(&["rye-aranet.service"]);
        let manager = FakeManager {
            stop_fails: true,
            ..FakeManager::default()
        };
        uninstall_with(&manager, ServiceManagerKind::Systemd, Some(dir.path())).unwrap();
        assert_eq!(
            manager.calls(),
            ["stop dev.rye.aranet", "uninstall dev.rye.aranet"]
        );
    }

    /// Reads the rendered unit's text only. It doesn't show what systemd does
    /// with these lines.
    #[test]
    fn unit_has_the_directives_the_refusal_reasons_and_docs_name() {
        let unit = render_systemd_unit("/usr/local/bin/aranet-service run");
        let lines: Vec<&str> = unit.lines().collect();

        // Each reason for a hidden directory starts with the directive that
        // hides it.
        for (dir, reason) in HIDDEN_FROM_SERVICE {
            let directive = reason.split(' ').next().unwrap();
            assert!(
                lines.contains(&directive),
                "{dir} is refused because of `{directive}`, which the unit doesn't set:\n{unit}"
            );
        }

        for expected in [
            // "the service can only write /var/lib/aranet" (a refused database),
            // and the rustdoc's "`UMask=0077` keeps both owner-only".
            "ProtectSystem=strict",
            "StateDirectory=aranet",
            "UMask=0077",
            // Nothing else would miss one of these if the renderer and the
            // shipped unit lost it together: without any one of them,
            // `systemd-analyze security` (systemd 252) still reports an
            // exposure under the 5.0 that the CI job allows.
            "EnvironmentFile=-/etc/default/aranet",
            "Group=aranet",
            "StateDirectoryMode=0750",
            "NoNewPrivileges=yes",
            "PrivateDevices=yes",
            "ProtectKernelTunables=yes",
            "ProtectKernelModules=yes",
            "ProtectControlGroups=yes",
        ] {
            assert!(
                lines.contains(&expected),
                "missing {expected:?} in:\n{unit}"
            );
        }

        // D-Bus (BlueZ), the HTTP API, and listing the interfaces for mDNS.
        let families = lines
            .iter()
            .find_map(|line| line.strip_prefix("RestrictAddressFamilies="))
            .expect("the unit restricts the address families");
        for family in ["AF_UNIX", "AF_INET", "AF_INET6", "AF_NETLINK"] {
            assert!(
                families.split(' ').any(|allowed| allowed == family),
                "RestrictAddressFamilies={families} leaves out {family}"
            );
        }
    }

    /// Checks that rely on Unix paths: on Windows `/usr/...` isn't absolute.
    #[cfg(unix)]
    mod unix_paths {
        use super::*;

        type Entry = (&'static str, u32, u32, u32);

        /// A `stat` that knows only `entries` of `(path, uid, gid, mode)`.
        fn fake_stat(entries: &[Entry]) -> impl Fn(&Path) -> std::io::Result<(u32, u32, u32)> + '_ {
            move |path| {
                entries
                    .iter()
                    .find(|(entry, ..)| Path::new(entry) == path)
                    .map(|&(_, uid, gid, mode)| (uid, gid, mode))
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
            }
        }

        /// `/`, `/usr`, `/usr/local`, `/usr/local/bin` (root, 0755) and `PROGRAM`
        /// (root, 0755), with `changes` replacing the entries of the same path.
        fn chain(changes: &[Entry]) -> Vec<Entry> {
            let mut entries = vec![
                ("/", 0, 0, 0o40755),
                ("/usr", 0, 0, 0o40755),
                ("/usr/local", 0, 0, 0o40755),
                ("/usr/local/bin", 0, 0, 0o40755),
                (PROGRAM, 0, 0, 0o100755),
            ];
            for change in changes {
                let entry = entries.iter_mut().find(|e| e.0 == change.0).unwrap();
                *entry = *change;
            }
            entries
        }

        #[test]
        fn writable_by_non_root_table() {
            let cases = [
                ("root 0755 chain", chain(&[]), None),
                (
                    "user-owned file",
                    chain(&[(PROGRAM, 501, 20, 0o100755)]),
                    Some("/usr/local/bin/aranet-service is owned by uid 501"),
                ),
                (
                    "0777 directory",
                    chain(&[("/usr/local/bin", 0, 0, 0o40777)]),
                    Some("/usr/local/bin is writable by every user"),
                ),
                (
                    "group-writable, gid 50",
                    chain(&[("/usr/local", 0, 50, 0o40775)]),
                    Some("/usr/local is writable by group 50"),
                ),
                (
                    "group-writable, gid 0",
                    chain(&[("/usr/local", 0, 0, 0o40775)]),
                    None,
                ),
            ];
            for (case, entries, expected) in cases {
                assert_eq!(
                    writable_by_non_root_with(Path::new(PROGRAM), fake_stat(&entries)).as_deref(),
                    expected,
                    "{case}"
                );
            }

            let entries = chain(&[]);
            let reason =
                writable_by_non_root_with(Path::new("/usr/local/bin/other"), fake_stat(&entries))
                    .expect("a stat error counts as writable");
            assert!(
                reason.starts_with("can't check /usr/local/bin/other: "),
                "{reason}"
            );
        }

        #[test]
        fn writable_by_non_root_on_real_user_file() {
            use std::os::unix::fs::MetadataExt;

            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("aranet-service");
            fs::write(&file, b"#!/bin/sh\n").unwrap();
            let uid = fs::metadata(&file).unwrap().uid();
            if uid == 0 {
                eprintln!("skipped: running as root, so the file is root-owned");
                return;
            }

            assert_eq!(
                writable_by_non_root(&file),
                Some(format!("{} is owned by uid {uid}", file.display()))
            );
        }

        #[test]
        fn not_runnable_by_service_table() {
            let cases = [
                ("0755 chain", PROGRAM, chain(&[]), None),
                (
                    "0700 file",
                    PROGRAM,
                    chain(&[(PROGRAM, 0, 0, 0o100700)]),
                    Some("/usr/local/bin/aranet-service is not executable by other users"),
                ),
                (
                    "0750 directory",
                    PROGRAM,
                    chain(&[("/usr/local/bin", 0, 0, 0o40750)]),
                    Some("/usr/local/bin is not searchable by other users"),
                ),
                (
                    "root's cargo install",
                    "/root/.cargo/bin/aranet-service",
                    chain(&[]),
                    Some("ProtectHome=yes hides /root from the service"),
                ),
                (
                    "/tmp",
                    "/tmp/aranet-service",
                    chain(&[]),
                    Some("PrivateTmp=yes gives the service its own /tmp"),
                ),
            ];
            for (case, program, entries, expected) in cases {
                assert_eq!(
                    not_runnable_by_service_with(Path::new(program), fake_stat(&entries))
                        .as_deref(),
                    expected,
                    "{case}"
                );
            }
        }

        #[test]
        fn system_args_table() {
            let options =
                |config: Option<&str>, database: Option<&str>| aranet_service::RunOptions {
                    config: config.map(PathBuf::from),
                    database: database.map(PathBuf::from),
                    ..aranet_service::RunOptions::default()
                };
            let refused = |run: aranet_service::RunOptions| match systemd_system_args(&run) {
                Err(ServiceError::InaccessiblePath { path, reason }) => (path, reason),
                other => panic!("{run:?}: {other:?}"),
            };

            assert_eq!(
                systemd_system_args(&aranet_service::RunOptions::default()).unwrap(),
                ["run", "--config", "/var/lib/aranet/server.toml"]
            );
            assert_eq!(
                systemd_system_args(&aranet_service::RunOptions {
                    bind: Some("0.0.0.0:9090".to_string()),
                    no_collector: true,
                    ..aranet_service::RunOptions::default()
                })
                .unwrap(),
                [
                    "run",
                    "--config",
                    "/var/lib/aranet/server.toml",
                    "--bind",
                    "0.0.0.0:9090",
                    "--no-collector",
                ]
            );
            assert_eq!(
                systemd_system_args(&options(
                    Some("/etc/aranet/server.toml"),
                    Some("/var/lib/aranet/a.db")
                ))
                .unwrap(),
                [
                    "run",
                    "--config",
                    "/etc/aranet/server.toml",
                    "--database",
                    "/var/lib/aranet/a.db",
                ]
            );

            assert_eq!(
                refused(options(Some("/home/u/x.toml"), None)),
                (
                    PathBuf::from("/home/u/x.toml"),
                    "ProtectHome=yes hides /home from the service"
                )
            );
            for database in ["/tmp/x.db", "/srv/x.db", "/var/lib/aranet/../x.db"] {
                assert_eq!(
                    refused(options(None, Some(database))),
                    (
                        PathBuf::from(database),
                        "the service can only write /var/lib/aranet"
                    )
                );
            }

            // A relative config is made absolute. Where the working directory is
            // under /home (GitHub's Linux runners), the result is then refused.
            let expected = env::current_dir().unwrap().join("server.toml");
            match systemd_system_args(&options(Some("server.toml"), None)) {
                Ok(args) => assert_eq!(args[2], expected.into_os_string()),
                Err(ServiceError::InaccessiblePath { path, .. }) => {
                    assert_eq!(path, expected);
                    assert!(hidden_from_service(&expected).is_some(), "{path:?}");
                }
                Err(e) => panic!("{e:?}"),
            }
        }

        #[test]
        fn program_checks_reach_the_root_directory_and_fail_closed() {
            // Whoever owns or can write a directory can replace everything
            // below it, so the check goes all the way up to `/`.
            let writable = |entries: &[Entry]| {
                writable_by_non_root_with(Path::new(PROGRAM), fake_stat(entries))
            };
            assert_eq!(
                writable(&chain(&[("/", 501, 0, 0o40755)])).as_deref(),
                Some("/ is owned by uid 501")
            );
            assert_eq!(
                writable(&chain(&[("/", 0, 0, 0o40757)])).as_deref(),
                Some("/ is writable by every user")
            );
            assert_eq!(
                writable(&chain(&[("/usr", 0, 50, 0o40775)])).as_deref(),
                Some("/usr is writable by group 50")
            );
            // A relative path doesn't say which directories to check.
            assert_eq!(
                writable_by_non_root_with(Path::new("bin/aranet-service"), fake_stat(&chain(&[])))
                    .as_deref(),
                Some("bin/aranet-service is not an absolute path")
            );

            let unrunnable = |program: &str, entries: &[Entry]| {
                not_runnable_by_service_with(Path::new(program), fake_stat(entries))
            };
            assert_eq!(
                unrunnable(PROGRAM, &chain(&[("/usr", 0, 0, 0o40750)])).as_deref(),
                Some("/usr is not searchable by other users")
            );
            assert_eq!(
                unrunnable(PROGRAM, &chain(&[("/", 0, 0, 0o40750)])).as_deref(),
                Some("/ is not searchable by other users")
            );
            let reason = unrunnable("/usr/local/bin/other", &chain(&[]))
                .expect("a stat error counts as unrunnable");
            assert!(
                reason.starts_with("can't check /usr/local/bin/other: "),
                "{reason}"
            );
        }

        #[test]
        fn sandbox_paths_are_matched_by_whole_components() {
            let hidden = [
                ("/home", "ProtectHome=yes hides /home from the service"),
                (
                    "/home/u/.cargo/bin/aranet-service",
                    "ProtectHome=yes hides /home from the service",
                ),
                (
                    "/root/aranet-service",
                    "ProtectHome=yes hides /root from the service",
                ),
                (
                    "/run/user/1000/aranet-service",
                    "ProtectHome=yes hides /run/user from the service",
                ),
                (
                    "/tmp/aranet-service",
                    "PrivateTmp=yes gives the service its own /tmp",
                ),
                (
                    "/var/tmp/aranet-service",
                    "PrivateTmp=yes gives the service its own /var/tmp",
                ),
            ];
            for (path, reason) in hidden {
                assert_eq!(hidden_from_service(Path::new(path)), Some(reason), "{path}");
            }
            // A name that only starts like a hidden directory's is another
            // directory, and so is a `tmp` further down.
            for path in [
                "/homes/u/aranet-service",
                "/rootfs/aranet-service",
                "/run/users/aranet-service",
                "/run/aranet-service",
                "/tmpfiles/aranet-service",
                "/var/tmpfiles/aranet-service",
                "/var/lib/aranet/tmp/server.toml",
                PROGRAM,
            ] {
                assert_eq!(hidden_from_service(Path::new(path)), None, "{path}");
            }

            // A config in a hidden directory is refused with that directory's reason.
            for (config, expected) in [
                (
                    "/run/user/1000/server.toml",
                    "ProtectHome=yes hides /run/user from the service",
                ),
                (
                    "/var/tmp/server.toml",
                    "PrivateTmp=yes gives the service its own /var/tmp",
                ),
            ] {
                let options = aranet_service::RunOptions {
                    config: Some(PathBuf::from(config)),
                    ..aranet_service::RunOptions::default()
                };
                match systemd_system_args(&options) {
                    Err(ServiceError::InaccessiblePath { path, reason }) => {
                        assert_eq!((path.as_path(), reason), (Path::new(config), expected));
                    }
                    other => panic!("{config}: {other:?}"),
                }
            }

            // The database must be in the state directory itself: not next to
            // it, and not in a directory whose name only starts like its name.
            let with_database = |database: &str| {
                systemd_system_args(&aranet_service::RunOptions {
                    database: Some(PathBuf::from(database)),
                    ..aranet_service::RunOptions::default()
                })
            };
            for database in ["/var/lib/aranet/data.db", "/var/lib/aranet/old/data.db"] {
                assert_eq!(
                    with_database(database).unwrap(),
                    [
                        "run",
                        "--config",
                        "/var/lib/aranet/server.toml",
                        "--database",
                        database,
                    ],
                    "{database}"
                );
            }
            for database in [
                "/var/lib/data.db",
                "/var/lib/aranet2/data.db",
                "/var/lib/aranet.db",
            ] {
                match with_database(database) {
                    Err(ServiceError::InaccessiblePath { path, reason }) => {
                        assert_eq!(path, Path::new(database));
                        assert_eq!(reason, "the service can only write /var/lib/aranet");
                    }
                    other => panic!("{database}: {other:?}"),
                }
            }
        }

        #[test]
        fn sandbox_paths_are_matched_with_symlinks_resolved_and_dot_dot_is_refused() {
            use std::os::unix::fs::symlink;

            const DOT_DOT: &str = "a path with `..` can't be checked against the directories the \
                                   unit hides; give it without `..`";

            // The roots live in a TempDir: `home` is a directory, `root` is a
            // symlink to `real-root` (Fedora Atomic links /home and /root into
            // /var like that), and `run/user` doesn't exist.
            let dir = tempfile::tempdir().unwrap();
            let base = dir.path();
            fs::create_dir_all(base.join("home/u")).unwrap();
            fs::write(base.join("home/u/server.toml"), "").unwrap();
            fs::create_dir(base.join("etc")).unwrap();
            fs::create_dir(base.join("real-root")).unwrap();
            fs::create_dir(base.join("real-root2")).unwrap();
            symlink(base.join("real-root"), base.join("root")).unwrap();
            // Into the hidden directory: a directory and a file. And out of it.
            symlink(base.join("home/u"), base.join("etc/link")).unwrap();
            symlink(
                base.join("home/u/server.toml"),
                base.join("etc/linked.toml"),
            )
            .unwrap();
            symlink(base.join("etc"), base.join("home/out")).unwrap();
            let roots = [
                (base.join("home"), "home is hidden"),
                (base.join("root"), "root is hidden"),
                (base.join("run/user"), "run/user is hidden"),
            ];

            let cases = [
                ("home/u/server.toml", Some("home is hidden")),
                // `..`, wherever it leads: into a hidden directory or not.
                ("etc/../home/u/server.toml", Some(DOT_DOT)),
                ("etc/../etc/server.toml", Some(DOT_DOT)),
                // Through a symlinked directory, also with directories that
                // don't exist yet, and through a symlinked file.
                ("etc/link/server.toml", Some("home is hidden")),
                ("etc/link/new/dir/server.toml", Some("home is hidden")),
                ("etc/linked.toml", Some("home is hidden")),
                // A hidden directory that is a symlink: by its name and by
                // the real path of what it points to.
                ("root/server.toml", Some("root is hidden")),
                ("real-root/server.toml", Some("root is hidden")),
                (
                    "real-root/.config/aranet/server.toml",
                    Some("root is hidden"),
                ),
                // As written it is in the hidden directory, which the service
                // can't pass through, wherever the symlink leads.
                ("home/out/server.toml", Some("home is hidden")),
                // A hidden directory that doesn't exist is compared as written.
                ("run/user/1000/server.toml", Some("run/user is hidden")),
                // Outside the hidden directories, also by a name that only
                // starts like a resolved one's.
                ("etc/server.toml", None),
                ("etc/new/dir/server.toml", None),
                ("real-root2/server.toml", None),
                ("run/users/server.toml", None),
            ];
            let wrong: Vec<String> = cases
                .iter()
                .filter_map(|(path, expected)| {
                    let found = hidden_under(&base.join(path), &roots);
                    (found != *expected)
                        .then(|| format!("{path}: found {found:?}, expected {expected:?}"))
                })
                .collect();
            assert!(wrong.is_empty(), "{wrong:#?}");

            // `service install --config` refuses `..` with the path as given.
            for config in ["/etc/../home/u/server.toml", "/etc/aranet/../server.toml"] {
                let error = systemd_system_args(&aranet_service::RunOptions {
                    config: Some(PathBuf::from(config)),
                    ..aranet_service::RunOptions::default()
                })
                .expect_err(config);
                assert!(
                    matches!(error, ServiceError::InaccessiblePath { .. }),
                    "{config}: {error:?}"
                );
                assert_eq!(
                    error.to_string(),
                    format!("{config} can't be used by the system service: {DOT_DOT}")
                );
            }
        }
    }
}
