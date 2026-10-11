//! Checks on the deployment files: `docker/Dockerfile`, `.dockerignore`,
//! `docker/server.toml` and the two compose files.
//!
//! The files live outside this crate, so these tests only run in a checkout of
//! the repository. They read the files line by line, without a Dockerfile or
//! YAML parser, so the compose files must stay in block style. The image itself
//! is built and started by `scripts/ci/check-docker-image.sh`, in the `docker`
//! job of `.github/workflows/deployment-files.yml`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aranet_service::middleware::RateLimitState;
use aranet_service::{AppState, Config, app};
use aranet_store::Store;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use tower::ServiceExt;

/// A path in the repository, relative to its root.
fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The contents of a repository file.
fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"))
}

/// The instructions of `docker/Dockerfile`, one string each: comment lines
/// dropped, continuation lines joined and whitespace runs collapsed to one
/// space.
fn dockerfile() -> Vec<String> {
    let text = read("docker/Dockerfile");
    let mut instructions = Vec::new();
    let mut current = String::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.strip_suffix('\\') {
            Some(start) => {
                current.push_str(start);
                current.push(' ');
            }
            None => {
                current.push_str(line);
                instructions.push(current.split_whitespace().collect::<Vec<_>>().join(" "));
                current.clear();
            }
        }
    }
    instructions
}

/// The arguments of the last `keyword` instruction, the one Docker uses, in
/// exec form (a JSON array of strings). `HEALTHCHECK … CMD` is not a `CMD`.
fn exec_form(instructions: &[String], keyword: &str) -> Vec<String> {
    let args = instructions
        .iter()
        .rev()
        .find_map(|instruction| {
            let (word, args) = instruction.split_once(' ')?;
            word.eq_ignore_ascii_case(keyword).then_some(args)
        })
        .unwrap_or_else(|| panic!("docker/Dockerfile has no {keyword} instruction"));
    serde_json::from_str(args).unwrap_or_else(|e| {
        panic!("{keyword} must be in exec form (a JSON array of strings), got `{args}`: {e}")
    })
}

/// `GET uri` with a `Host` header; returns the status and the body.
async fn get(app: &Router, uri: &str, host: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .uri(uri)
        .header(header::HOST, host)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[test]
fn dockerfile_entrypoint_reads_baked_config() {
    let entrypoint = exec_form(&dockerfile(), "ENTRYPOINT");
    assert!(
        entrypoint
            .windows(2)
            .any(|pair| pair == ["--config", "/app/server.toml"]),
        "ENTRYPOINT must pass --config /app/server.toml, so the arguments of \
         `docker run <image> …` replace only CMD and keep the config: {entrypoint:?}"
    );
}

#[test]
fn dockerfile_bakes_docker_server_toml() {
    let copies: Vec<String> = dockerfile()
        .into_iter()
        .filter(|instruction| instruction.starts_with("COPY "))
        .collect();
    assert!(
        copies
            .iter()
            .any(|copy| copy == "COPY --chmod=0644 docker/server.toml /app/server.toml"),
        "the image must contain docker/server.toml as /app/server.toml, mode 0644 \
         whatever its mode in the checkout, so that the service (uid 999) can read \
         it: {copies:#?}"
    );
}

#[test]
fn dockerfile_cmd_has_no_runtime_overrides() {
    let cmd = exec_form(&dockerfile(), "CMD");
    let overrides: Vec<&String> = cmd
        .iter()
        .filter(|arg| {
            ["--bind", "-b", "--database", "-d"].contains(&arg.as_str())
                || arg.starts_with("--bind=")
                || arg.starts_with("--database=")
        })
        .collect();
    assert!(
        overrides.is_empty(),
        "CMD overrides /app/server.toml with {overrides:?}; `docker run <image> \
         --no-collector` would also drop them: {cmd:?}"
    );
}

#[test]
fn dockerfile_builds_the_whole_workspace_locked() {
    let instructions = dockerfile();
    assert!(
        instructions
            .iter()
            .any(|instruction| instruction == "COPY . ."),
        "the builder must copy the whole workspace (`COPY . .`): Cargo loads every \
         member's manifest and targets, even to build one package"
    );
    assert!(
        instructions.iter().any(|instruction| {
            instruction.starts_with("RUN ")
                && instruction
                    .contains("cargo build --release --locked -p aranet-service --features full")
        }),
        "the builder must run `cargo build --release --locked -p aranet-service \
         --features full`"
    );
    let ignored: Vec<&String> = instructions
        .iter()
        .filter(|instruction| instruction.contains("exit 0"))
        .collect();
    assert!(
        ignored.is_empty(),
        "a failing build step must fail the image build: {ignored:?}"
    );
}

#[test]
fn dockerignore_excludes_nested_targets() {
    let text = read(".dockerignore");
    let patterns: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    for pattern in ["**/target/", ".claude/"] {
        assert!(
            patterns.contains(&pattern),
            ".dockerignore must list {pattern}, or `COPY . .` sends nested target \
             directories and worktrees to the builder: {patterns:?}"
        );
    }
}

#[tokio::test]
async fn docker_default_config_is_valid_and_serves_metrics() {
    let config = Config::load(repo("docker/server.toml"))
        .unwrap_or_else(|e| panic!("docker/server.toml: {e}"));
    if let Err(e) = config.validate() {
        panic!("the container would exit at startup: {e}");
    }
    assert_eq!(
        config.server.bind, "0.0.0.0:8080",
        "EXPOSE, HEALTHCHECK and the compose files assume port 8080 on every interface \
         of the container"
    );
    assert_eq!(
        config.storage.path,
        Path::new("/data/aranet.db"),
        "the data volume is mounted at /data"
    );

    // The config path is in a temporary directory, so no handler can write
    // docker/server.toml. `t` outlives the router, which holds the state.
    let t = tempfile::tempdir().unwrap();
    let security = Arc::new(config.security.clone());
    let rate_limit = Arc::new(RateLimitState::new(&config.security));
    let state = AppState::with_config_path(
        Store::open_in_memory().unwrap(),
        config,
        t.path().join("server.toml"),
    );
    // Prometheus scrapes from another container on the compose network.
    let app = app(state, security, rate_limit)
        .layer(MockConnectInfo(SocketAddr::from(([172, 18, 0, 3], 45678))));

    // docker/prometheus.yml scrapes the target aranet-service:8080.
    let (status, body) = get(&app, "/metrics", "aranet-service:8080").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /metrics from Prometheus: {body}"
    );
    assert!(body.contains("aranet_collector_running"), "{body}");

    // The HEALTHCHECK and the compose healthchecks use 127.0.0.1:8080.
    let (status, body) = get(&app, "/api/health", "127.0.0.1:8080").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /api/health from the healthcheck: {body}"
    );
}

/// The compose files, relative to the repository root.
const COMPOSE_FILES: [&str; 2] = ["docker-compose.yml", "docker/docker-compose.yml"];

/// The directory a compose file's relative paths start from.
fn compose_dir(file: &str) -> PathBuf {
    repo(file)
        .parent()
        .expect("a compose file's directory")
        .to_path_buf()
}

/// Indentation of `line`, in bytes.
fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// `line` without its YAML comment: a whole-line `#` comment, or one that
/// starts with ` #`.
fn strip_comment(line: &str) -> &str {
    let code = if line.trim_start().starts_with('#') {
        ""
    } else {
        line.split(" #").next().unwrap_or(line)
    };
    code.trim_end()
}

/// The items of every indented block list under a `key:` line (such as each
/// service's `ports:`), as written after `- `. Comments are skipped, and
/// top-level keys (the named `volumes:` table) are not lists.
fn list_items(text: &str, key: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().map(strip_comment).collect();
    let mut items = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(rest) = line.trim_start().strip_prefix(key) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        if indent(line) == 0 {
            continue;
        }
        assert!(
            rest.trim().is_empty(),
            "`{}`: write {key} as a block list, one `- ` item per line",
            line.trim()
        );
        let found = items.len();
        items.extend(
            lines[i + 1..]
                .iter()
                .filter(|line| !line.trim().is_empty())
                .take_while(|line| indent(line) > indent(lines[i]))
                .filter_map(|line| line.trim().strip_prefix("- "))
                .map(|item| item.trim().to_string()),
        );
        assert!(
            items.len() > found,
            "`{}` has no `- ` items indented under it",
            line.trim()
        );
    }
    items
}

/// The Dockerfile of every `build:` in a compose file: `build: <context>`
/// (with `Dockerfile` in the context) or a block with `context:` and
/// `dockerfile:`, which is relative to the context.
fn compose_dockerfiles(file: &str) -> Vec<PathBuf> {
    let text = read(file);
    let lines: Vec<&str> = text.lines().map(strip_comment).collect();
    let value = |v: &str| v.trim().trim_matches(['"', '\'']).to_string();
    let mut dockerfiles = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(rest) = line.trim_start().strip_prefix("build:") else {
            continue;
        };
        let (context, dockerfile) = if rest.trim().is_empty() {
            let block: Vec<&str> = lines[i + 1..]
                .iter()
                .filter(|line| !line.trim().is_empty())
                .take_while(|line| indent(line) > indent(lines[i]))
                .map(|line| line.trim())
                .collect();
            let context = block
                .iter()
                .find_map(|entry| entry.strip_prefix("context:"))
                .unwrap_or_else(|| panic!("{file}: `build:` has no `context:`"));
            let dockerfile = block
                .iter()
                .find_map(|entry| entry.strip_prefix("dockerfile:"))
                .unwrap_or("Dockerfile");
            (value(context), value(dockerfile))
        } else {
            assert!(
                !rest.trim().starts_with('{'),
                "{file}: write `build:` as a block"
            );
            (value(rest), "Dockerfile".to_string())
        };
        dockerfiles.push(compose_dir(file).join(context).join(dockerfile));
    }
    dockerfiles
}

#[test]
fn root_dockerfile_is_gone() {
    assert!(
        !repo("Dockerfile").exists(),
        "docker/Dockerfile is the only Dockerfile; build with \
         `docker build -f docker/Dockerfile .`"
    );
}

#[test]
fn compose_files_are_unprivileged_and_loopback_only() {
    let mut problems = Vec::new();
    for file in COMPOSE_FILES {
        let text = read(file);
        for (n, line) in text.lines().enumerate() {
            let words = line.split_whitespace().collect::<Vec<_>>().join(" ");
            let words = words.replace(['"', '\''], "");
            if words.contains("privileged: true") || words.contains("network_mode: host") {
                problems.push(format!("{file}:{}: `{}`", n + 1, line.trim()));
            }
        }
        let ports = list_items(&text, "ports");
        if ports.is_empty() {
            problems.push(format!("{file}: no `ports:` list"));
        }
        for port in ports {
            if !port.starts_with("\"127.0.0.1:") {
                problems.push(format!(
                    "{file}: {port} is not published on \"127.0.0.1:…\""
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "Bluetooth needs only the host's /run/dbus, mounted read-only, and ports are \
         published on 127.0.0.1 only:\n{}",
        problems.join("\n")
    );
}

#[test]
fn compose_bind_mount_sources_exist() {
    let mut missing = Vec::new();
    for file in COMPOSE_FILES {
        for volume in list_items(&read(file), "volumes") {
            let volume = volume.trim_matches(['"', '\'']);
            let source = volume.split(':').next().unwrap_or_default();
            let relative = source.starts_with("./") || source.starts_with("../");
            if relative && !compose_dir(file).join(source).exists() {
                missing.push(format!("{file}: {volume}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "bind mount sources that don't exist (Docker would create an empty directory \
         in their place):\n{}",
        missing.join("\n")
    );
}

#[test]
fn compose_build_dockerfiles_exist() {
    for file in COMPOSE_FILES {
        let dockerfiles = compose_dockerfiles(file);
        assert!(!dockerfiles.is_empty(), "{file} builds no image");
        for dockerfile in dockerfiles {
            assert!(
                dockerfile.is_file(),
                "{file} builds from {}, which doesn't exist",
                dockerfile.display()
            );
        }
    }
}
