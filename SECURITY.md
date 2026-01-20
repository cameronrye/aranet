# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | Yes                |

## Reporting a Vulnerability

We take security vulnerabilities seriously. If you discover a security issue, please report it responsibly.

### How to Report

**Do NOT create a public GitHub issue for security vulnerabilities.**

Instead, please report security issues by emailing:

**security@rye.dev**

Include the following information:

1. **Description** of the vulnerability
2. **Steps to reproduce** the issue
3. **Affected versions** and components
4. **Potential impact** assessment
5. **Any suggested fixes** (optional but appreciated)

### What to Expect

- **Acknowledgment**: We will acknowledge receipt within 48 hours
- **Initial Assessment**: Within 7 days, we will provide an initial assessment
- **Updates**: We will keep you informed of our progress
- **Resolution**: We aim to resolve critical issues within 30 days
- **Credit**: We will credit you in the release notes (unless you prefer anonymity)

### Scope

This security policy applies to:

- `aranet-core` - Core BLE library
- `aranet-types` - Shared types
- `aranet-store` - Local data persistence
- `aranet-service` - Background collector and REST API
- `aranet-cli` - Command-line interface
- `aranet-tui` - Terminal dashboard

### Out of Scope

- Vulnerabilities in third-party dependencies (please report to the upstream project)
- Issues that require physical access to the user's device
- Social engineering attacks
- Issues in development/test code only

## Security Best Practices

When using the Aranet ecosystem:

1. **Keep software updated**: Always use the latest version
2. **Secure your network**: The REST API binds to localhost by default
3. **Protect your database**: The SQLite database contains sensor history
4. **Review permissions**: On Linux, Bluetooth requires appropriate permissions

## Dependency Security

We use the following tools to maintain dependency security:

- `cargo audit` - Check for known vulnerabilities in dependencies
- `cargo outdated` - Track outdated dependencies
- Dependabot alerts - Automated security updates

## Past Security Issues

No security issues have been reported to date.

---

Made with ❤️ by [Cameron Rye](https://rye.dev/)

