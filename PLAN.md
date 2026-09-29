# PLAN

## Goal

Build a Rust-based Linux adapter that mounts NAVER MYBOX storage as a filesystem device by using the official MYBOX OpenAPI:
<https://developers.mybox.naver.com/>

## Implementation Plan

### 1) Scope and Requirements

- [ ] Target modern Linux distributions with FUSE support.
- [ ] Mount MYBOX into a local directory and support common file operations (`ls`, `cat`, `cp`, `mv`, `rm`, `mkdir`).
- [ ] Keep full POSIX compatibility (special files, hard links, advanced ACLs) out of the first release.
- [ ] Keep an offline synchronization daemon out of the first release.

### 2) High-Level Architecture

- [x] **CLI Layer:** Provide `myboxfs mount <mountpoint>`, `myboxfs unmount <mountpoint>`, and `myboxfs login` / token management helpers.
- [ ] **FUSE Filesystem Layer:** Implement core FUSE operations (`lookup`, `getattr`, `readdir`, `open`, `read`, `write`, `create`, `unlink`, `rename`, `mkdir`, `rmdir`, `flush`, `release`).
- [ ] **MYBOX API Client Layer:** Build a typed Rust client for OpenAPI endpoints with request retries, timeout handling, and error mapping.
- [ ] **Auth/Token Layer:** Implement the OAuth/OpenAPI token acquisition flow, secure local token storage, and token refresh flow.
- [ ] **Metadata & Cache Layer:** Implement in-memory inode/path mapping, directory listing and attribute caches with TTL, and an optional temporary file cache for write-back.

### 3) Suggested Rust Crates

- [x] Use `fuser` for FUSE integration.
- [ ] Use `tokio` and `reqwest` for the async runtime and HTTP client.
- [x] Use `serde` with reqwest's JSON support for API responses.
- [ ] Use `thiserror` and/or `anyhow` for error handling.
- [x] Use `tracing` and `tracing-subscriber` for logging.
- [x] Use `clap` for CLI parsing.
- [ ] Use `keyring` or an encrypted local file approach for credential storage.

### 4) Data and Operation Design

- [x] Manage path-to-inode mapping in adapter memory.
- [ ] Translate remote object metadata into Linux file attributes (size, mtime, file/directory type, and synthetic default permissions).
- [x] For reads, resolve the inode/path, fetch data from the MYBOX API, and return the requested byte range.
- [ ] For writes, buffer data or use a temporary file, upload on `flush`/`release` (or use chunked upload if supported), and invalidate parent directory and file attribute caches.

### 5) Error Handling and Reliability

- [ ] Map API and network failures consistently to Linux errno values.
- [ ] Implement exponential backoff for transient errors (429, 5xx, and timeouts).
- [ ] Add request correlation IDs to logs.
- [ ] Ensure graceful unmount on fatal runtime errors.

### 6) Security Plan

- [ ] Never log access tokens or sensitive headers.
- [ ] Store tokens securely, preferring the system keyring.
- [ ] Enforce HTTPS and validate certificates.
- [ ] Validate and sanitize paths to prevent traversal bugs.

### 7) Development Milestones

- [ ] **MVP Mount (Read-Only)**
  - [x] Implement login/token loading.
  - [x] Implement `getattr`, `readdir`, `open`, and `read`.
  - [ ] Verify file reads against the live MYBOX API after the range-read change.
- [ ] **Read-Write Support**
  - [x] Implement file `create`, `write`, `flush`, and `unlink` with upload on flush/close.
  - [ ] Implement `rename`, `mkdir`, and `rmdir`.
  - [ ] Verify create, edit, and delete against a live MYBOX account.
- [ ] **Caching & Performance**
  - [ ] Tune attribute and directory cache TTLs.
  - [ ] Improve large-file reads and writes (range/chunk support; ranged reads implemented, live verification pending).
- [ ] **Hardening**
  - [ ] Add retry policies, robust error mapping, and better observability.
- [ ] **Packaging**
  - [ ] Build and release binaries and provide distro installation guidance.

### 8) Testing Strategy

- [ ] **Unit tests:** Cover path/inode translation, API response mapping, and cache invalidation logic.
- [ ] **Integration tests:** Use a mocked MYBOX API server for deterministic FUSE behavior and test mounting plus basic filesystem operations in CI/container.
- [ ] **Manual validation:** Run real-account smoke tests (`mount`, browse, upload, rename, delete, unmount).

### 9) Operational Considerations

- [ ] Use `~/.config/myboxfs/config.toml` as the config file location.
- [x] Provide structured logs for troubleshooting (download diagnostics are debug-build-only).
- [ ] Consider an optional metrics endpoint for request latency and cache hit ratios.

## Immediate Next Steps

- [x] Initialize the Rust project structure (`cli`, `fuse`, `api`, `auth`, `cache` modules).
- [x] Implement token entry/loading and a minimal API health check command.
- [x] Deliver the first read-only FUSE mount prototype and validate it with basic file listing.
- [ ] Confirm ranged reads of large files against MYBOX and whether its download server honors HTTP Range.
