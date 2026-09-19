# Security and data-safety boundaries

RECCH is a privileged database client, not a sandbox for untrusted databases, SQL scripts or AI-generated queries. A passing build is not a certification that every vulnerability has been eliminated.

## Data handling

Connection settings and AI settings are stored locally as JSON. Updates use an OS file lock and atomic replacement; malformed configuration is rejected without overwriting it. Newly written configuration and dump temporary files are owner-only on Unix. **This is not encryption or an OS keychain.** Protect the operating-system account, disk and backups; Windows protection depends on directory ACLs. A process running as the same user or administrator can still access credentials. Never attach configuration files or real credentials to public issues.

AI is optional and requires explicit consent for each generation. The prompt and selected table/column schema are sent to the configured provider; do not include confidential row data in a prompt. Remote endpoints must use HTTPS; HTTP is allowed only for loopback addresses. Redirects are rejected. Request deadlines and response-size limits are enforced. An AI response is only inserted into the editor, never automatically executed. Review it as untrusted SQL. Database connections also leave the device when connecting to a remote server. Redis connections currently do not expose TLS configuration; use a trusted local endpoint or a separately managed secure tunnel rather than sending credentials over an untrusted network.

## Export and restore

Database export now requires a compatible native `pg_dump` (prefer the server's major version) or Oracle MySQL 8 `mysqldump` on PATH. Native dumps avoid passing grid preview values through a lossy serializer. MySQL database export rejects non-InnoDB base tables instead of suggesting a consistent snapshot is available. Avoid concurrent DDL during a backup. A failed, timed-out or cancelled process does not replace an existing backup. The temporary destination is persisted only after successful process exit and file synchronization.

These are logical database dumps, not physical or cluster-wide backups. PostgreSQL ownership and privileges are deliberately excluded; roles, globals, replication state and disaster recovery need a separate DBA procedure. Native tools do not provide a reliable per-row progress percentage, so the UI shows an in-progress indicator. Test a restore in a separate environment before relying on a backup. The app does not execute psql shell/client commands, MySQL DELIMITER directives or COPY-from-stdin client streams; native tools are required for such scripts and large restores.

Grid results are limited to 10,000 rows / 32 MiB, with an explicit error rather than a silently truncated result. A single exceptionally large driver packet or Redis collection element can still consume memory before this application-level limit. Redis browsing lists up to 1,000 keys and previews up to 100 collection members or 64 KiB of string data. It is not an exhaustive inventory or backup; SCAN is not a transactionally consistent snapshot.

Big integers and decimals are returned as exact strings; JSON columns retain raw JSON text and binary values retain complete hexadecimal bytes. Unsupported types and duplicate result-column names fail explicitly: cast unsupported values or give distinct SQL aliases. Table JSON import rejects numeric tokens that JavaScript would round; encode exact numeric columns as strings and JSON columns as raw JSON text when required. CSV has no unambiguous NULL/empty-string distinction.

Table CSV/JSON imports use one transaction for PostgreSQL and InnoDB, with limits of 10,000 rows and 16 MiB of generated SQL. Triggers, external side effects, nontransactional side tables and sequence increments are outside rollback guarantees. A connection loss during COMMIT has an uncertain outcome: inspect the database before retrying. Arbitrary whole-database SQL imports are different: they can commit DDL or contain explicit transaction statements, so partial changes may remain on error. Import only trusted scripts and make a verified backup first.

Raw console and SQL-script sessions are not returned to shared pools, preventing USE, SELECT, SET or unfinished transactions from leaking into other operations. Console session state is not retained across separate executions. Visual PostgreSQL edits group rename/type/default/nullability/comment changes in a transaction. MySQL generated/automatic-update column attributes unsupported by the editor cause an explicit refusal rather than silent attribute removal; use reviewed SQL for advanced DDL.

## Verification and unresolved work

The regression suite includes pure SQL/CSV tests, actual Vue reactive setup tests with mocked IPC, configuration concurrency tests, process-failure tests and opt-in tests against disposable MySQL 8, PostgreSQL 16 and Redis 7 services. A Linux WebDriver smoke test checks the real WebView/IPC path. Platform unit builds do not replace Windows/macOS GUI tests. Real production data, every server version, complex custom types, replica failover, disk exhaustion, power failure and every extension combination are not exhaustively tested.

Dependency audit findings must be assessed separately from functional CI. Retain and disclose the current RustSec findings, especially advisories without a compatible upstream patch. Do not blanket-ignore advisories or describe a green test run as a clean security audit. Review installed native dump tools, system WebViews and OS updates separately. Repository branch protection and release signing are administrative controls and are not automatically established by these code changes.
