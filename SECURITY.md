# Security and verification boundaries

RECCH is a privileged local database client. Passing regression tests is not a zero-risk security certification. This document describes the implementation in PR #4, not the separate, unmerged PR #2 candidate.

## Credentials and transport

Database passwords and AI API keys remain plaintext in local JSON files. The configuration writer uses a sibling temporary file, file locking and replacement only after a successful write. On Unix, configuration files are restricted to mode 0600. These protections are **not encryption**. Windows confidentiality depends on the user's directory ACLs. OS credential-store integration and migration/recovery tests remain open work.

Redis currently connects over TCP without a configurable TLS option. Use only trusted networks or separately secured tunnels. MySQL/PostgreSQL connection defaults are not a guarantee of certificate/hostname verification; explicit CA and verified-TLS configuration still needs implementation and negative-certificate tests.

The AI feature transmits the prompt and selected table/column metadata to the configured provider only after the UI's consent checkbox is selected. It does not automatically execute returned SQL. Remote AI endpoints require HTTPS; HTTP is allowed only for explicitly configured loopback hosts. Redirects are disabled, requests time out, and responses are limited to 1 MiB. Local configuration storage does not mean that AI requests stay on the device. The current consent is not bound to an immutable endpoint snapshot and is not an authorization boundary against a compromised renderer.

## Database operations

Arbitrary SQL and Redis commands intentionally run with the connected account's privileges. Use least-privilege accounts. SQL query invocations use disposable checked-out sessions; Redis operations use separate physical connections so SELECT/MULTI/AUTH state cannot be shared across UI requests. This means session state is not preserved between separate Execute clicks.

CSV/JSON table imports use one transaction, with a 10000-row and 32 MiB generated-SQL bound. MySQL requires an InnoDB base table. An acknowledged statement failure rolls back that transaction. A connection failure at COMMIT is reported as an uncertain result: inspect the database before retrying. Triggers writing to nontransactional stores, external side effects, and sequence gaps are not undone by database transaction rollback.

Whole-database SQL script import is different: arbitrary scripts can contain explicit commits or implicit-commit DDL. Do not assume the entire script rolls back on failure. The importer is not mysql or psql and does not implement their client-side directives. Restore only trusted scripts into an isolated target first.

## Backups and value fidelity

Database SQL exports write to a temporary sibling file and replace an existing destination only after success. Failure/cancellation must not remove the previous backup. Locks coordinate cooperating RECCH operations; they are not protection against a malicious user replacing filesystem entries in the destination directory. Select a directory under your control.

The built-in exporter is a limited logical table exporter, not a verified complete disaster-recovery system. It rejects several unsupported advanced objects, but this is not an exhaustive inventory of every database feature. Ownership, privileges, custom collation/sequence properties, extension objects, and complex restore dependencies still require native tooling and restore verification. Do not use a RECCH export as your sole backup. The separate native-backup implementation proposed in PR #2 has not been merged by PR #4.

Large integer and decimal columns are exposed as text when needed to preserve digits; binary values are not truncated into previews for database SQL export. Unsupported decodes and duplicate result-column names fail explicitly. Binary-column grid mutations and text imports/exports are blocked rather than treating hex previews as original bytes.

CSV cannot unambiguously distinguish SQL NULL from an empty string. Grid JSON/SQL interchange still needs fully typed round-trip coverage for scalar JSON, JSON null versus SQL NULL, and embedded high-precision JSON numbers. Use tested native database backup/restore for fidelity-critical transfers.

Interactive SQL results and whole-table text exports still use fetch_all and can consume substantial memory. A configurable streaming result budget and real cancellation/large-result tests remain outstanding. The Redis key browser has a 10000-key bound; large key values and full SQL scripts still require resource-limit review.

## Verification and remaining work

CI runs frontend regression tests, TypeScript/Vite production builds, npm advisory checks, and Rust compile/unit tests on Linux, Windows and macOS. A separate job runs explicitly ignored integration tests against disposable MySQL 8.0, PostgreSQL 16 and Redis 7 services. These fixtures are not business databases.

Cross-platform compilation is not desktop GUI end-to-end verification. Packaging, signing/notarization, restore drills, crash/power-loss behavior, server-version compatibility and hostile-network testing are separate acceptance steps. No installers are published by these checks.

Rust dependencies require a fresh advisory scan of the final lockfile. Advisory counts recorded for the unmerged PR #2 lockfile must not be attributed to PR #4. Keep issue #3 open for dependency remediations, credential-store integration, verified database transport and release hardening. Do not suppress advisories or bypass failed checks merely to produce a green build.
