//! Checks on the deployment files: `docker/Dockerfile`, `.dockerignore`,
//! `docker/server.toml` and the two compose files.
//!
//! The files live outside this crate, so these tests only run in a checkout of
//! the repository. They read the files line by line, without a Dockerfile or
//! YAML parser, so the compose files must stay in block style. The image itself
//! is built and started by `scripts/ci/check-docker-image.sh`, in the `docker`
//! job of `.github/workflows/deployment-files.yml`. The last three tests here
//! read that workflow, that script and `scripts/ci/check-systemd-install.sh`
//! (run by the workflow's `systemd` job), as text too: they pin some of their
//! lines and run none of them.

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

// The tests below pin lines that the image check doesn't reach, or reaches
// only with Docker: each fails for a one-line change that the tests above let
// through.

/// Whether `instruction`, one of `dockerfile()`'s, is a `keyword` instruction.
/// Docker reads instruction names without regard to case, so `user root` is a
/// `USER` instruction too.
fn is_instruction(instruction: &str, keyword: &str) -> bool {
    instruction
        .split(' ')
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case(keyword))
}

#[test]
fn dockerfile_user_entrypoint_and_cmd_are_the_image_contract() {
    let instructions = dockerfile();
    let users: Vec<&String> = instructions
        .iter()
        .filter(|instruction| is_instruction(instruction, "USER"))
        .collect();
    assert_eq!(
        users,
        ["USER aranet"],
        "the service must not run as root, and the last USER instruction is the \
         one that counts"
    );
    let useradd = instructions
        .iter()
        .find(|instruction| instruction.contains("useradd "))
        .expect("docker/Dockerfile creates no user");
    for part in [
        "groupadd --system --gid 999 aranet ",
        "useradd --system --uid 999 --gid 999 ",
        " --home-dir /data ",
    ] {
        assert!(
            useradd.contains(part),
            "`{part}` is missing: uid and gid 999 keep existing aranet-data volumes \
             writable, and the home directory is the data volume: {useradd}"
        );
    }
    assert!(
        useradd.ends_with(" && chown aranet:aranet /data"),
        "/data must belong to the service user, or it can't create its database \
         in a new volume: {useradd}"
    );

    assert_eq!(
        exec_form(&instructions, "ENTRYPOINT"),
        ["/app/aranet-service", "--config", "/app/server.toml"],
        "ENTRYPOINT is the service binary and its config, nothing else"
    );
    assert_eq!(
        exec_form(&instructions, "CMD"),
        ["run"],
        "the default command is `run`: with `--no-collector` as the default the \
         image would start, answer every check and never poll a sensor"
    );
}

#[test]
fn dockerfile_pins_its_base_images_and_uses_the_built_in_frontend() {
    let stages: Vec<String> = dockerfile()
        .into_iter()
        .filter(|instruction| is_instruction(instruction, "FROM"))
        .collect();
    assert_eq!(
        stages,
        [
            "FROM rust:1.90-slim-bookworm AS builder",
            "FROM debian:bookworm-slim"
        ],
        "the builder names its Rust version and both stages the same Debian \
         release, so the binary finds the libraries it was linked against"
    );
    // Docker reads a parser directive without regard to case or to spaces and
    // tabs around its name, so `#syntax=…` and `# SYNTAX = …` count too.
    let directives: Vec<String> = read("docker/Dockerfile")
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix('#'))
        .map(|comment| comment.replace([' ', '\t'], "").to_ascii_lowercase())
        .filter(|comment| comment.starts_with("syntax="))
        .collect();
    assert!(
        directives.is_empty(),
        "a `# syntax=` line, however it is spelled, would pull an unpinned \
         Dockerfile frontend image; BuildKit's built-in frontend already supports \
         the cache mounts and `COPY --chmod`: {directives:?}"
    );
}

#[test]
fn dockerfile_run_steps_stop_at_the_first_failing_command() {
    let unchecked: Vec<String> = dockerfile()
        .into_iter()
        .filter(|instruction| {
            is_instruction(instruction, "RUN") && instruction.contains([';', '|'])
        })
        .collect();
    assert!(
        unchecked.is_empty(),
        "RUN steps chain their commands with `&&` only. After `;`, `||` or a pipe a \
         failed command no longer fails the image build: after a failed `cargo \
         build`, `cp` would copy an older binary out of the cache mount: \
         {unchecked:#?}"
    );
}

#[test]
fn dockerignore_excludes_env_files_coverage_output_docs_and_the_website() {
    let text = read(".dockerignore");
    let patterns: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    for pattern in [
        "**/.env*",
        "**/*.profraw",
        "**/*.profdata",
        "coverage/",
        "lcov.info",
    ] {
        assert!(
            patterns.contains(&pattern),
            ".dockerignore must list {pattern}, or `COPY . .` sends a developer's \
             environment files or coverage output to the builder (a bare `.env*` \
             matches only the context root): {patterns:?}"
        );
    }
    // docker/README.md has users keep a writable copy of the config at the
    // context root, and the service writes the API key it generates into it.
    // Patterns are anchored at the context root, so this one leaves
    // docker/server.toml, which the image bakes in.
    assert!(
        patterns.contains(&"server.toml"),
        ".dockerignore must list server.toml, or `COPY . .` sends a config kept at \
         the repository root, which can hold an API key, to the builder, and every \
         sensor edit rebuilds that layer: {patterns:?}"
    );
    // docker/Dockerfile's comment on `COPY . .` says that .dockerignore keeps
    // the docs out.
    for pattern in ["docs/", "website/"] {
        assert!(
            patterns.contains(&pattern),
            ".dockerignore must list {pattern}, or `COPY . .` sends the docs or the \
             website to the builder, and with the website a developer's \
             website/node_modules: {patterns:?}"
        );
    }
}

#[test]
fn expose_healthchecks_and_published_port_are_loopback_8080() {
    // The port that docker/server.toml binds, which
    // `docker_default_config_is_valid_and_serves_metrics` pins.
    let url = "http://127.0.0.1:8080/api/health";

    let instructions = dockerfile();
    assert!(
        instructions
            .iter()
            .any(|instruction| instruction == "EXPOSE 8080"),
        "docker/Dockerfile must EXPOSE 8080"
    );
    // The last one, which is the one Docker uses.
    let healthcheck = instructions
        .iter()
        .rfind(|instruction| is_instruction(instruction, "HEALTHCHECK"))
        .expect("docker/Dockerfile has no HEALTHCHECK");
    assert!(
        healthcheck.ends_with(&format!(" CMD curl -fsS {url} || exit 1")),
        "the image's HEALTHCHECK must ask {url}. Nothing else checks it: the image \
         check never reads a container's health, and the compose files replace it: \
         {healthcheck}"
    );
    let runtime = instructions
        .rsplit(|instruction| is_instruction(instruction, "FROM"))
        .next()
        .unwrap_or_default();
    let installed: Vec<&str> = runtime
        .iter()
        .filter(|instruction| {
            is_instruction(instruction, "RUN") && instruction.contains("apt-get install ")
        })
        .flat_map(|instruction| instruction.split(' '))
        .collect();
    for (package, why) in [
        (
            "libdbus-1-3",
            "the binary is linked against libdbus, which is how it reaches BlueZ",
        ),
        (
            "libssl3",
            "the binary is linked against OpenSSL, which the MQTT client uses",
        ),
        (
            "ca-certificates",
            "without it the MQTT client can't verify a broker's certificate, and \
             no check makes a TLS connection, so nothing else would notice",
        ),
        ("curl", "every healthcheck runs it inside the container"),
    ] {
        assert!(
            installed.contains(&package),
            "the final stage must install {package}: {why}: {runtime:#?}"
        );
    }

    for file in COMPOSE_FILES {
        let text = read(file);
        let probes: Vec<&str> = text
            .lines()
            .map(|line| strip_comment(line).trim())
            .filter(|line| line.starts_with("test:"))
            .collect();
        let probe = format!("test: [\"CMD\", \"curl\", \"-fsS\", \"{url}\"]");
        assert!(
            probes.contains(&probe.as_str()),
            "{file}: the healthcheck of aranet-service must be `{probe}`: {probes:?}"
        );
        let ports = list_items(&text, "ports");
        assert!(
            ports.iter().any(|port| port == "\"127.0.0.1:8080:8080\""),
            "{file} must publish the API as \"127.0.0.1:8080:8080\": {ports:?}"
        );
    }
}

#[test]
fn compose_files_mount_the_data_volume_and_the_config_and_dbus_read_only() {
    let expected: [(&str, &[&str]); 2] = [
        (
            "docker-compose.yml",
            &[
                "aranet-data:/data",
                "./docker/server.toml:/app/server.toml:ro",
                "/run/dbus:/run/dbus:ro",
            ],
        ),
        (
            "docker/docker-compose.yml",
            &["aranet-data:/data", "./server.toml:/app/server.toml:ro"],
        ),
    ];
    for (file, mounts) in expected {
        let text = read(file);
        let volumes = list_items(&text, "volumes");
        for mount in mounts {
            assert!(
                volumes.iter().any(|volume| volume == mount),
                "{file} must mount {mount}: the database lives in /data, and the \
                 service reads /app/server.toml and must not write it: {volumes:#?}"
            );
        }
        // Every D-Bus mount, including the commented one in docker/, which is
        // there to be uncommented.
        let dbus: Vec<&str> = text
            .lines()
            .map(|line| line.trim().trim_start_matches('#').trim())
            .filter(|line| line.starts_with("- ") && line.contains("dbus"))
            .collect();
        assert_eq!(
            dbus,
            ["- /run/dbus:/run/dbus:ro"],
            "{file}: the host's system bus is mounted read-only"
        );
    }
}

#[test]
fn compose_files_set_privileged_to_nothing_but_false() {
    let mut problems = Vec::new();
    for file in COMPOSE_FILES {
        for (n, line) in read(file).lines().enumerate() {
            // Comments count too, as in
            // `compose_files_are_unprivileged_and_loopback_only`.
            let code = line.trim().trim_start_matches('#').trim_start();
            let Some(value) = code.strip_prefix("privileged:") else {
                continue;
            };
            let value = strip_comment(value).trim().trim_matches(['"', '\'']);
            if !value.eq_ignore_ascii_case("false") {
                problems.push(format!("{file}:{}: `{}`", n + 1, line.trim()));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "compose reads `True`, `TRUE`, `yes` and `on` as true as well, so the only \
         value `privileged:` may have in these files is false:\n{}",
        problems.join("\n")
    );
}

/// `docker stop` and `docker compose down` kill a container that is still
/// running after 10 s, unless its service sets `stop_grace_period`. The
/// service's own shutdown can take 20 s: 5 s for the collector's tasks to
/// stop, 5 s for a Bluetooth poll in flight to disconnect, and 10 s for the
/// background tasks, which write what they still hold.
#[test]
fn compose_files_give_the_service_time_to_shut_down() {
    for file in COMPOSE_FILES {
        let text = read(file);
        let lines: Vec<&str> = text.lines().map(strip_comment).collect();
        let at = lines
            .iter()
            .position(|line| *line == "  aranet-service:")
            .unwrap_or_else(|| panic!("{file}: no `aranet-service:` service"));
        // The service's own keys: one level below its name.
        let grace: Vec<&str> = lines[at + 1..]
            .iter()
            .filter(|line| !line.trim().is_empty())
            .take_while(|line| indent(line) > indent(lines[at]))
            .filter(|line| indent(line) == indent(lines[at]) + 2)
            .filter_map(|line| line.trim().strip_prefix("stop_grace_period:"))
            .map(|value| value.trim().trim_matches(['"', '\'']))
            .collect();
        assert_eq!(
            grace,
            ["30s"],
            "{file}: the aranet-service service must set `stop_grace_period: 30s`. With \
             Docker's default of 10 s the service is killed in the middle of its shutdown"
        );
    }
}

#[test]
fn deployment_workflow_runs_the_image_check_for_main_and_deploy_file_changes() {
    let text = read(".github/workflows/deployment-files.yml");
    let lines: Vec<&str> = text
        .lines()
        .map(|line| strip_comment(line).trim())
        .collect();
    assert!(
        lines.contains(&"run: scripts/ci/check-docker-image.sh"),
        "a job must run scripts/ci/check-docker-image.sh, the only check that \
         builds and starts the image"
    );
    let push = lines
        .iter()
        .position(|line| *line == "push:")
        .expect("the workflow has no push trigger");
    assert_eq!(
        lines.get(push + 1),
        Some(&"branches: [main]"),
        "every push to main must run the workflow"
    );
    let paths = list_items(&text, "paths");
    for path in [
        "crates/**",
        "docker/**",
        "docker-compose.yml",
        ".dockerignore",
        "Cargo.toml",
        "Cargo.lock",
        "distribution/systemd/**",
        "scripts/ci/**",
        ".github/workflows/deployment-files.yml",
    ] {
        assert!(
            paths.contains(&format!("\"{path}\"")),
            "a pull request that changes {path} must run the workflow: {paths:?}"
        );
    }

    // The token, the checkout's credentials and the action pins. zizmor reports
    // a change to any of them, but no workflow runs it. Only the top-level
    // `permissions:` is read: a job that asks for more in a block of its own
    // passes.
    let raw: Vec<&str> = text.lines().map(strip_comment).collect();
    let permissions: Vec<&str> = raw
        .iter()
        .skip_while(|line| **line != "permissions:")
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .take_while(|line| indent(line) > 0)
        .map(|line| line.trim())
        .collect();
    assert_eq!(
        permissions,
        ["contents: read"],
        "the workflow's top-level `permissions:` must be a block with `contents: \
         read` and nothing else. With more, or without the block (the token then \
         gets the repository's default permissions), a job's token could write"
    );
    let mut checkouts = 0;
    for (i, line) in raw.iter().enumerate() {
        let entry = line.trim_start();
        let key = entry.strip_prefix("- ").map_or(entry, str::trim_start);
        let Some(action) = key.strip_prefix("uses:") else {
            continue;
        };
        let action = action.trim().trim_matches(['"', '\'']);
        assert!(
            action.rsplit_once('@').is_some_and(|(_, commit)| {
                commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
            }),
            "`uses: {action}` must name a commit by its SHA, 40 hexadecimal digits \
             after `@`: a tag or a branch can be moved to other code"
        );
        if !action.to_ascii_lowercase().starts_with("actions/checkout@") {
            continue;
        }
        checkouts += 1;
        // The rest of the step: the lines indented at least as far as `uses`.
        let column = line.len() - key.len();
        let step: Vec<&str> = raw[i + 1..]
            .iter()
            .filter(|line| !line.trim().is_empty())
            .take_while(|line| indent(line) >= column)
            .map(|line| line.trim())
            .collect();
        assert!(
            step.contains(&"persist-credentials: false"),
            "the checkout step must set `persist-credentials: false` below its \
             `uses:` line, or the checkout keeps the token where the job's later \
             steps can use it: {step:?}"
        );
    }
    assert!(
        checkouts > 0,
        "no step uses actions/checkout, so no job has the script to run"
    );
}

/// The lines after `start` up to the next line that is `end`, without the empty ones.
fn block<'a>(lines: &[&'a str], start: &str, end: &str) -> Vec<&'a str> {
    let at = lines
        .iter()
        .position(|line| *line == start)
        .unwrap_or_else(|| panic!("no `{start}` line"));
    lines[at + 1..]
        .iter()
        .copied()
        .take_while(|line| *line != end)
        .filter(|line| !line.is_empty())
        .collect()
}

/// Pins, in the text of `scripts/ci/check-docker-image.sh`, the things whose
/// loss would be worst. A failed check ends the script as a failure: it runs
/// under `set -euo pipefail`, `fail` ends with `exit 1`, and its EXIT trap
/// exits with the status the script was ending with. And container ports are
/// published on 127.0.0.1 only. It reads the script and runs nothing, so it is
/// not a test of what the script checks: only a run against a broken image is.
#[test]
fn image_check_script_keeps_exit_1_in_fail_and_loopback_only_ports() {
    if !repo("crates/aranet-service/Cargo.toml").is_file() {
        return; // not a repository checkout: there is no scripts/ci/ to read
    }
    let script = read("scripts/ci/check-docker-image.sh");
    // The script's comments are whole lines or start with ` #`, like the YAML
    // comments `strip_comment` is for.
    let lines: Vec<&str> = script
        .lines()
        .map(|line| strip_comment(line).trim())
        .collect();

    // Without errexit a failed command that no `|| fail` follows would not stop
    // the script. After the `docker compose … config -q` of a broken compose
    // file, it would go on to print that every check passed.
    assert!(
        lines.contains(&"set -euo pipefail"),
        "the script must run under `set -euo pipefail`"
    );
    // The EXIT trap replaces the script's exit status with its own: ending in
    // `exit 0`, or in a command that succeeds, it would turn every failed check
    // into a success.
    let cleanup = block(&lines, "cleanup() {", "}");
    assert_eq!(
        (cleanup.first(), cleanup.last()),
        (Some(&"local status=$?"), Some(&r#"exit "$status""#)),
        "the EXIT trap must end with the status the script was exiting with: {cleanup:#?}"
    );
    assert!(
        lines.contains(&"trap cleanup EXIT"),
        "the script must install `cleanup` as its EXIT trap"
    );

    // The script's own checks stop it through `fail`. With `exit 0` there, a
    // failed check would end the script, and the job, as a success.
    let start = lines
        .iter()
        .position(|line| *line == "fail() {")
        .expect("the script has no `fail() {` line");
    let body: Vec<&str> = lines[start + 1..]
        .iter()
        .copied()
        .take_while(|line| *line != "}")
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(
        body.last(),
        Some(&"exit 1"),
        "`fail` must end with `exit 1`. With `exit 0` there, a failed check would \
         end the script as a success: {body:#?}"
    );

    // Every port that a `docker run` or `docker create` publishes: `-p` and
    // `--publish` with their address, `-P` and `--publish-all` with none.
    // Continuation lines are joined first.
    let commands = lines.join("\n").replace("\\\n", " ");
    let mut published = Vec::new();
    for command in commands.lines() {
        let words: Vec<&str> = command.split_whitespace().collect();
        if !words.iter().any(|word| word.ends_with("docker"))
            || !(words.contains(&"run") || words.contains(&"create"))
        {
            continue;
        }
        for (i, word) in words.iter().enumerate() {
            // A group of short options, such as `-p` or `-dp`.
            let short = word.strip_prefix('-').filter(|letters| {
                !letters.is_empty() && letters.bytes().all(|byte| byte.is_ascii_alphabetic())
            });
            let address =
                if *word == "--publish" || short.is_some_and(|letters| letters.ends_with('p')) {
                    words.get(i + 1).copied().unwrap_or_default()
                } else if word.starts_with("--publish-all")
                    || short.is_some_and(|letters| letters.contains('P'))
                {
                    ""
                } else if let Some(address) = word
                    .strip_prefix("--publish=")
                    .or_else(|| word.strip_prefix("-p"))
                {
                    address
                } else {
                    continue;
                };
            published.push((address.trim_matches(['"', '\'']), command));
        }
    }
    assert!(
        !published.is_empty(),
        "the script publishes no container port with `-p` or `--publish`. If it \
         reaches its containers another way now, this test must check that way"
    );
    let open: Vec<&(&str, &str)> = published
        .iter()
        .filter(|(address, _)| !address.starts_with("127.0.0.1:"))
        .collect();
    assert!(
        open.is_empty(),
        "the script must publish container ports on 127.0.0.1 only, as in `-p \
         127.0.0.1::8080`: the service in its containers has no authentication, \
         and `-p 8080` would put it on every interface of the machine that runs \
         the script: {open:#?}"
    );
}

/// Pins, by text, what lets the workflow's two service jobs and
/// `scripts/ci/check-systemd-install.sh` fail: the jobs build the binary and
/// run their checks on the right system with the read-only token, and the
/// script stops at a failed check, keeps the exposure threshold, requires the
/// dynamic user, treats an accepted home copy as an error and limits the time
/// of every `curl` call that `curl_calls` finds. It reads the files and runs
/// nothing, so it is not a test of the service, of systemd or of launchd: only
/// the jobs themselves are.
#[test]
fn service_jobs_run_their_checks_and_the_systemd_script_can_fail() {
    /// The lines of the job `name`, trimmed: those indented deeper than its
    /// `  <name>:` line.
    fn job<'a>(workflow: &[&'a str], name: &str) -> Vec<&'a str> {
        let key = format!("  {name}:");
        let start = workflow
            .iter()
            .position(|line| *line == key)
            .unwrap_or_else(|| panic!("the workflow has no `{name}` job"));
        workflow[start + 1..]
            .iter()
            .filter(|line| !line.trim().is_empty())
            .take_while(|line| indent(line) > 2)
            .map(|line| line.trim())
            .collect()
    }

    /// Every `curl` call on `line`, from the command name to the end of that
    /// command: an unquoted `|`, `;`, `&`, `)` or backtick, or the end of the
    /// line. `>&` and `&>` are redirections and end nothing.
    ///
    /// `curl` is a call wherever it stands as a word of its own, so also
    /// after `$(`, a backtick, `;`, `|`, `&&`, `!`, a quote or a directory
    /// (`/usr/bin/curl`). It is not one inside a longer name such as
    /// `libcurl`, `curl-config` or `$curl`. A call through a variable or
    /// `eval`, and another client such as `wget`, are not found. This reads
    /// one line at a time, so a call continued on the next line ends here.
    /// Quotes are followed from the command name on: for a call that is
    /// itself inside a quoted string, the command may end later than found.
    fn curl_calls(line: &str) -> Vec<&str> {
        let in_name = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '$');
        let mut calls = Vec::new();
        for (at, name) in line.match_indices("curl") {
            let rest = &line[at + name.len()..];
            if line[..at].chars().next_back().is_some_and(in_name)
                || rest.chars().next().is_some_and(in_name)
            {
                continue;
            }
            let mut quote = None;
            let mut previous = ' ';
            let mut end = rest.len();
            let mut chars = rest.char_indices().peekable();
            while let Some((i, c)) = chars.next() {
                let next = chars.peek().map(|&(_, next)| next);
                match quote {
                    Some(open) if c == open => quote = None,
                    Some(_) => {}
                    None if matches!(c, '"' | '\'') => quote = Some(c),
                    None if c == '&' && (matches!(previous, '>' | '<') || next == Some('>')) => {}
                    None if matches!(c, '|' | ';' | '&' | ')' | '`') => {
                        end = i;
                        break;
                    }
                    None => {}
                }
                previous = c;
            }
            calls.push(line[at..at + name.len() + end].trim_end());
        }
        calls
    }

    // The forms `curl_calls` must find, and the names it must leave alone.
    let forms: [(&str, &[&str]); 9] = [
        ("curl -fsS http://a", &["curl -fsS http://a"]),
        (
            r#"body=$(curl -fsS "$url") || fail "no answer""#,
            &[r#"curl -fsS "$url""#],
        ),
        (r#"echo "$(curl -fsS "$url")""#, &[r#"curl -fsS "$url""#]),
        ("body=`curl -fsS http://a`", &["curl -fsS http://a"]),
        (
            "true; curl -a | curl -b && curl -c || ! curl -d",
            &["curl -a", "curl -b", "curl -c", "curl -d"],
        ),
        (
            r#"if /usr/bin/curl -fsS "$url" > /dev/null 2>&1; then"#,
            &[r#"curl -fsS "$url" > /dev/null 2>&1"#],
        ),
        (
            r#"curl -fsS "http://a/?b=1&c=(2);d" &> /dev/null --max-time 5 &"#,
            &[r#"curl -fsS "http://a/?b=1&c=(2);d" &> /dev/null --max-time 5"#],
        ),
        ("(curl)", &["curl"]),
        (
            "ldd /usr/lib/libcurl.so; curl-config --version; echo $curl .curlrc",
            &[],
        ),
    ];
    for (line, calls) in forms {
        assert_eq!(curl_calls(line), calls, "{line}");
    }

    if !repo("crates/aranet-service/Cargo.toml").is_file() {
        return; // not a repository checkout: there is no workflow or script to read
    }
    let text = read(".github/workflows/deployment-files.yml");
    let workflow: Vec<&str> = text.lines().map(strip_comment).collect();

    let systemd = job(&workflow, "systemd");
    assert!(
        systemd
            .iter()
            .any(|line| line.starts_with("runs-on: ubuntu-")),
        "the systemd job needs a runner whose init system is systemd: {systemd:#?}"
    );
    assert!(
        systemd
            .contains(&"run: sudo scripts/ci/check-systemd-install.sh target/debug/aranet-service"),
        "the systemd job must run scripts/ci/check-systemd-install.sh as root on the \
         binary it built, the only check that installs and starts the system service: \
         {systemd:#?}"
    );

    let launchd = job(&workflow, "launchd-refusal");
    assert!(
        launchd
            .iter()
            .any(|line| line.starts_with("runs-on: macos-")),
        "only a macOS runner has launchd, whose system-level install must be refused: \
         {launchd:#?}"
    );
    // The job's three checks. Each must end the step with `exit 1`.
    for check in [
        r#"if sudo ./target/debug/aranet-service service install 2> "$err_file"; then"#,
        r#"if ! grep -qF -- '--user' "$err_file"; then"#,
        "if [ -e /Library/LaunchDaemons/dev.rye.aranet.plist ]; then",
    ] {
        assert_eq!(
            block(&launchd, check, "fi").last(),
            Some(&"exit 1"),
            "the launchd-refusal job must fail when `{check}` holds"
        );
    }

    for (name, lines) in [("systemd", &systemd), ("launchd-refusal", &launchd)] {
        assert!(
            lines.contains(&"run: cargo build --locked -p aranet-service"),
            "the {name} job must build the binary it checks, with the committed \
             Cargo.lock: {lines:#?}"
        );
        assert!(
            lines.contains(&r#"toolchain: "1.90""#),
            "the {name} job must build with Rust 1.90, the MSRV: {lines:#?}"
        );
        assert!(
            !lines.iter().any(|line| line.starts_with("permissions:")),
            "the {name} job must keep the workflow's read-only token: a `permissions:` \
             block of its own could give its steps, which run `sudo`, a token that writes"
        );
    }

    let text = read("scripts/ci/check-systemd-install.sh");
    let script: Vec<&str> = text
        .lines()
        .map(|line| strip_comment(line).trim())
        .collect();

    // Without errexit and pipefail a failing command, such as `systemd-analyze
    // security` over its threshold in front of `| tail`, would not stop the script.
    assert!(
        script.contains(&"set -euo pipefail"),
        "the script must run under `set -euo pipefail`"
    );
    assert_eq!(
        block(&script, "fail() {", "}").last(),
        Some(&"exit 1"),
        "`fail` must end with `exit 1`. With `exit 0` there, a failed check would end \
         the script as a success"
    );
    // The EXIT trap replaces the script's exit status with its own.
    let on_exit = block(&script, "on_exit() {", "}");
    assert_eq!(
        (on_exit.first(), on_exit.last()),
        (Some(&"local status=$?"), Some(&r#"exit "$status""#)),
        "the EXIT trap must end with the status the script was exiting with: {on_exit:#?}"
    );
    assert!(
        script.contains(&"trap on_exit EXIT"),
        "the script must install `on_exit` as its EXIT trap"
    );

    let scored: Vec<&&str> = script
        .iter()
        .filter(|line| line.starts_with("systemd-analyze security "))
        .collect();
    assert!(
        !scored.is_empty() && scored.iter().all(|line| line.contains(" --threshold=50 ")),
        "`systemd-analyze security` must run with `--threshold=50`, an exposure of at \
         most 5.0: {scored:#?}"
    );
    assert!(
        script
            .iter()
            .any(|line| line.starts_with(r#"[ "$user" = aranet ] || fail "#)),
        "the script must fail unless the service's main process runs as `aranet`"
    );
    assert!(
        block(
            &script,
            r#"if "$home_copy" service install 2> "$work/refusal.err"; then"#,
            "fi"
        )
        .first()
        .is_some_and(|line| line.starts_with("fail ")),
        "the script must fail when `service install` accepts the copy of the binary \
         in the invoking user's home directory"
    );

    // A connection that is accepted and never answered would otherwise hold
    // the job until its own timeout. Each call needs its own limit: one on
    // another command of the same line doesn't count.
    let curls: Vec<&str> = script.iter().flat_map(|line| curl_calls(line)).collect();
    assert!(
        !curls.is_empty(),
        "the script has no `curl` call. If it reaches the service another way now, \
         this test must require that way's time limit"
    );
    for call in curls {
        let words: Vec<&str> = call.split_whitespace().collect();
        let limit = words
            .iter()
            .position(|word| *word == "--max-time")
            .and_then(|at| words.get(at + 1))
            .and_then(|seconds| seconds.parse::<u32>().ok());
        assert!(
            limit.is_some_and(|seconds| seconds > 0),
            "every `curl` call must have a time limit of its own, on its line, as in \
             `--max-time 5`. Without one, a connection that is accepted and never \
             answered holds the job until its 30-minute timeout: `{call}`"
        );
    }
}
