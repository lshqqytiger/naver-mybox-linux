# naver-mybox-linux

Unofficial NAVER MYBOX Adapter for Linux.

## Goal

Build a Rust-based Linux adapter that mounts NAVER MYBOX storage as a filesystem device by using the official MYBOX OpenAPI:
<https://developers.mybox.naver.com/>

## Implementation Plan

### 1) Scope and Requirements

- Target platform: modern Linux distributions with FUSE support.
- Primary capability: mount MYBOX into a local directory and support common file operations (`ls`, `cat`, `cp`, `mv`, `rm`, `mkdir`).
- Non-goals for first release:
  - Full POSIX compatibility (special files, hard links, advanced ACLs).
  - Offline synchronization daemon.

### 2) High-Level Architecture

- **CLI Layer**
  - `myboxfs mount <mountpoint>`
  - `myboxfs unmount <mountpoint>`
  - `myboxfs login` / token management helpers
- **FUSE Filesystem Layer**
  - Implement core FUSE operations (`lookup`, `getattr`, `readdir`, `open`, `read`, `write`, `create`, `unlink`, `rename`, `mkdir`, `rmdir`, `flush`, `release`).
- **MYBOX API Client Layer**
  - Typed Rust client for OpenAPI endpoints.
  - Request retry, timeout handling, and error mapping.
- **Auth/Token Layer**
  - OAuth/OpenAPI token acquisition flow.
  - Secure local token storage and refresh flow.
- **Metadata & Cache Layer**
  - In-memory inode/path mapping.
  - Directory listing cache and attribute cache with TTL.
  - Optional temporary file cache for write-back.

### 3) Suggested Rust Crates

- `fuser` (FUSE integration)
- `tokio` + `reqwest` (async runtime and HTTP client)
- `serde` / `serde_json` (serialization)
- `thiserror` / `anyhow` (error handling)
- `tracing` / `tracing-subscriber` (logging)
- `clap` (CLI parsing)
- `keyring` or encrypted local file approach (credential storage)

### 4) Data and Operation Design

- Path ↔ inode mapping managed in adapter memory.
- Remote object metadata translated into Linux file attributes:
  - size, mtime, file/dir type, permissions (synthetic defaults).
- Read path:
  1. Resolve inode/path
  2. Fetch metadata/data from cache or MYBOX API
  3. Return requested byte range
- Write path:
  1. Buffer/temporary write
  2. Upload on `flush/release` (or chunked upload if supported)
  3. Invalidate parent directory cache and file attr cache

### 5) Error Handling and Reliability

- Map API/network failures to Linux errno values consistently.
- Implement exponential backoff for transient errors (429/5xx/timeouts).
- Add request correlation IDs in logs.
- Ensure graceful unmount on fatal runtime errors.

### 6) Security Plan

- Never log access tokens or sensitive headers.
- Store tokens securely (system keyring preferred).
- Enforce HTTPS and validate certificates.
- Validate and sanitize path handling to avoid traversal bugs.

### 7) Development Milestones

1. **MVP Mount (Read-Only)**
   - Login/token load
   - `getattr`, `readdir`, `open`, `read`
2. **Read-Write Support**
   - `create`, `write`, `flush`, `unlink`, `rename`, `mkdir`, `rmdir`
3. **Caching & Performance**
   - Attr/dir cache TTL tuning
   - Large-file read/write improvements (range/chunk)
4. **Hardening**
   - Retry policies, robust error mapping, better observability
5. **Packaging**
   - Build/release binaries and provide distro installation guidance

### 8) Testing Strategy

- **Unit tests**
  - Path/inode translation
  - API response mapping
  - Cache invalidation logic
- **Integration tests**
  - Mocked MYBOX API server for deterministic FUSE behavior
  - Mount + basic filesystem operations in CI/container
- **Manual validation**
  - Real account smoke tests (`mount`, browse, upload, rename, delete, unmount)

### 9) Operational Considerations

- Config file location: `~/.config/myboxfs/config.toml`
- Structured logs for troubleshooting.
- Optional metrics endpoint for request latency/cache hit ratios.

## Immediate Next Steps

1. Initialize Rust project structure (`cli`, `fuse`, `api`, `auth`, `cache` modules).
2. Implement token acquisition/loading and a minimal API health check command.
3. Deliver first read-only FUSE mount prototype and validate with basic file listing.
