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
cargo test --workspace -- --ignored
```

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
   cargo clippy --workspace -- -D warnings
   cargo fmt --check
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

## License

By contributing, you agree that your contributions will be licensed under the MIT License.

## Questions?

If you have questions about contributing, feel free to open a discussion or reach out via issues.

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)

