# Contributing to Aranet

Thank you for your interest in contributing to the Aranet project! This document provides guidelines and information to help you get started.

## Code of Conduct

By participating in this project, you agree to maintain a respectful and inclusive environment for everyone.

## Getting Started

### Prerequisites

- **Rust 1.90+** (Edition 2024)
- **Bluetooth adapter** with BLE support
- Platform-specific dependencies:
  - **Linux**: `libdbus-1-dev` (for BlueZ)
  - **macOS**: Xcode Command Line Tools
  - **Windows**: No additional dependencies

### Building from Source

```bash
git clone https://github.com/cameronrye/aranet.git
cd aranet
cargo build --workspace
```

### Running Tests

```bash
# Run all tests
cargo test --workspace

# Run tests with hardware (requires Aranet device)
ARANET_DEVICE="Aranet4 12345" cargo test --workspace -- --ignored
```

The CLI integration tests (`crates/aranet-cli/tests/cli_integration.rs`) run the
`aranet` binary against a temporary config and data directory, so they never read
or change your own. For the hardware tests, set `ARANET_DEVICE` to a device address
or name: aliases from your config are not visible to them.

## How to Contribute

### Reporting Bugs

Before submitting a bug report:

1. Check existing [issues](https://github.com/cameronrye/aranet/issues) to avoid duplicates
2. Gather relevant information:
   - Aranet device model and firmware version
   - Operating system and version
   - Rust version (`rustc --version`)
   - Full error message and stack trace

Use the bug report issue template when creating a new issue.

### Suggesting Features

Feature requests are welcome! Please:

1. Check the [Architecture docs](docs/ARCHITECTURE.md) for technical details
2. Search existing issues for similar suggestions
3. Use the feature request issue template
4. Provide clear use cases and examples

### Submitting Changes

1. **Fork** the repository
2. **Create a branch** from `main`:
   ```bash
   git checkout -b feature/your-feature-name
   ```
3. **Make your changes** following the coding standards below
4. **Write or update tests** for your changes
5. **Run the full test suite**:
   ```bash
   cargo test --workspace
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo fmt --all --check
   ```
6. **Commit** with a clear message:
   ```bash
   git commit -m "Add feature: brief description"
   ```
7. **Push** to your fork and create a **Pull Request**

## Coding Standards

### Rust Style

- Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- Use `cargo fmt` for consistent formatting
- Address all `cargo clippy` warnings
- Write documentation for public APIs

### Commit Messages

- Use present tense ("Add feature" not "Added feature")
- Keep the first line under 72 characters
- Reference issues when applicable: `Fixes #123`

### Documentation

- Add doc comments (`///`) for all public items
- Include examples in documentation when helpful
- Update README.md if adding new features

### Testing

- Write unit tests for new functionality
- Add integration tests for BLE operations (mark with `#[ignore]`)
- Use property-based testing (proptest) for parsers
- Aim for good coverage of edge cases

## Project Structure

```
aranet/
├── crates/
│   ├── aranet-types/    # Platform-agnostic types (shared)
│   ├── aranet-core/     # Core BLE library
│   ├── aranet-store/    # Local SQLite data persistence
│   ├── aranet-service/  # Background collector and REST API
│   ├── aranet-cli/      # CLI tool
│   ├── aranet-tui/      # Terminal dashboard
│   └── aranet-gui/      # Desktop GUI
├── docs/                # Protocol documentation
├── distribution/        # Service configuration files
└── website/             # Documentation website
```

## Releasing (maintainers)

All seven crates share one version: `version` under `[workspace.package]` in the root `Cargo.toml`.
Releases are cut with [cargo-release](https://github.com/crate-ci/cargo-release) 1.1.6; its settings are in
`release.toml` and in `[package.metadata.release]` of `crates/aranet-cli/Cargo.toml`. A release is one
`chore: release vX.Y.Z` commit on `main`, one `vX.Y.Z` tag on exactly that commit, and crates published
from that tag.

You need cargo-release 1.1.6 (`cargo install cargo-release --version 1.1.6 --locked`, or the binary from
its GitHub release), `gh` logged in, and a crates.io token that can publish the seven crates (`cargo login`).
Publishing builds every crate, including aranet-gui, so use a Mac or a Linux machine with the GUI build
dependencies.

1. Check that `CHANGELOG.md` lists every change under `## [Unreleased]`.
2. On a release branch, bump the version and date the changelog, then open a pull request:
   ```bash
   git switch main && git pull --ff-only
   git switch -c release/v0.2.1
   cargo release 0.2.1 --no-publish --no-tag --no-push             # dry run: read what it would change
   cargo release 0.2.1 --no-publish --no-tag --no-push --execute   # commits "chore: release v0.2.1"
   git push -u origin release/v0.2.1
   gh pr create --fill
   ```
3. When CI is green, merge the pull request (rebase or squash), and merge nothing else until step 4 is done.
4. Tag the release commit and push the tag. This starts the Release workflow (binaries, installers,
   GitHub Release, Homebrew tap):
   ```bash
   git switch main && git pull --ff-only
   git log -1 --format=%s | grep -E '^chore: release v0\.2\.1( \(#[0-9]+\))?$'   # must print the subject
   cargo release tag --execute
   git push origin v0.2.1
   ```
   If the `grep` prints nothing, `main` has moved past the release commit: tag that commit instead with
   `git tag -a v0.2.1 -m "chore: release v0.2.1" <release commit>` and push the tag.
5. When the Release workflow for the tag has succeeded (`gh run list --workflow release.yml --branch v0.2.1`),
   publish the crates from the tag. cargo-release publishes them in dependency order and skips any
   version already on crates.io, so it is safe to re-run:
   ```bash
   git switch --detach v0.2.1
   cargo release publish --allow-branch HEAD             # dry run
   cargo release publish --allow-branch HEAD --execute
   git switch main
   ```
6. Update the website's release information (`website/src/site-config.mjs` and
   `website/src/content/docs/docs/changelog.mdx`) in a follow-up pull request.

Release tags are protected: they can't be moved or deleted. If the Release workflow fails after the tag is
pushed, nothing has reached crates.io yet; fix the cause on `main` and release the next patch version.

### Pre-releases

To run the whole release pipeline without publishing anything to crates.io or the Homebrew tap, cut a
release candidate from `main` on a throwaway branch. Never merge that branch: the real release starts
again from `main` at step 2, so its pull request holds only the `chore: release vX.Y.Z` commit.

```bash
git switch main && git pull --ff-only
git switch -c release/v0.2.1-rc.1
cargo release 0.2.1-rc.1 --no-publish --no-push             # dry run
cargo release 0.2.1-rc.1 --no-publish --no-push --execute   # commit and tag v0.2.1-rc.1; CHANGELOG untouched
git push origin release/v0.2.1-rc.1 v0.2.1-rc.1
```

The Release workflow publishes a GitHub pre-release; the Homebrew tap is not updated and nothing goes to
crates.io. The tag also starts the screenshot workflow, which may push a `screenshots/v0.2.1-rc.1` branch.
When you are done, remove all of it (pre-release tags, `v*-*`, are not protected):

```bash
gh release delete v0.2.1-rc.1 --cleanup-tag --yes
git push origin --delete release/v0.2.1-rc.1
git ls-remote --exit-code origin refs/heads/screenshots/v0.2.1-rc.1 && git push origin --delete screenshots/v0.2.1-rc.1
git switch main && git branch -D release/v0.2.1-rc.1 && git tag -d v0.2.1-rc.1
```

Then start the real release at step 2, from `main`.

## License

By contributing, you agree that your contributions will be licensed under the MIT License.

## Questions?

If you have questions about contributing, feel free to open a discussion or reach out via issues.

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)

