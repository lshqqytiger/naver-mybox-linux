# Implementation Plan

## Goal and Scope

Mount NAVER MYBOX on Linux using the [official OpenAPI](https://developers.mybox.naver.com/).
Support browsing, reading, creating, editing, deleting, moving, and renaming files and folders.
Use personal access tokens and FUSE; full POSIX semantics, special files, advanced ACLs,
and offline synchronization are out of scope for the first release.

Complete the steps in order. Check a step only after its verification criteria pass.

## 1. Establish the Current Baseline

- [x] Provide `login`, `health-check`, `mount`, and `unmount` commands with owner-only token storage.
- [x] Mount with FUSE and map remote entries to in-memory inodes and file attributes.
- [x] Browse directories and read requested byte ranges from files.
- [x] Create, edit, truncate, and delete files; stage writes in memory and upload on flush/close.
- [x] Add debug-build-only download diagnostics and unit tests for the local file lifecycle.

Verification: `cargo test`, `cargo fmt --check`, and `cargo check --release` pass.

## 2. Verify the Existing Workflow Against MYBOX

- [ ] With a test account, mount and unmount; browse root and nested directories.
- [ ] Read small and large files, including nonzero offsets; check whether the download server honors HTTP Range and whether reads time out.
- [ ] Create an empty file, upload a new file, overwrite and truncate an existing file, then delete a file to trash. Verify content and listings after remounting.
- [ ] Record actual API response shapes and error statuses; fix mismatches before adding more operations.

Verification: a real-account smoke run completes without lost data or stale entries. Never put tokens or signed URLs in test logs.

## 3. Add Deterministic API and FUSE Tests

- [ ] Mock the MYBOX upload-URL, multipart upload, metadata listing, download, and delete requests; assert request methods, bodies, and failure handling.
- [ ] Test mount-level create, read, write, truncate, and unlink flows in a FUSE-capable CI/container environment.
- [ ] Cover duplicate names, missing files, interrupted transfers, retries after flush failure, and inode/listing consistency.

Verification: tests run without MYBOX credentials and catch remote API contract changes.

## 4. Finish Filesystem Operations

- [ ] Implement folder creation and removal (`mkdir`, `rmdir`) with correct empty-folder and error behavior.
- [ ] Implement rename and move for files and folders, including cross-directory moves and collision rules.
- [ ] Update in-memory parent/child mappings after each successful remote mutation; preserve local state on failure.
- [ ] Return appropriate Linux errors for unsupported operations, missing entries, conflicts, and permission failures.

Verification: `mkdir`, `rmdir`, `mv`, `rm`, `cp`, `cat`, and `ls` work in the mocked mount and a real-account smoke run.

## 5. Harden Writes and Network Failures

- [ ] Replace whole-file in-memory edit buffers with bounded temporary storage or chunked uploads where the API supports them.
- [ ] Define persistence semantics for concurrent open handles, `fsync`, `flush`, and failed close; prevent a failed upload from silently losing edits.
- [ ] Map API/network failures to errno consistently and retry only safe transient requests (429, 5xx, timeouts) with bounded backoff.
- [ ] Track file size and modification time from remote metadata; invalidate directory and attribute caches after mutations.

Verification: large-file edits, concurrent writes, interrupted uploads, and remounts retain the expected content and metadata.

## 6. Improve Performance and Security

- [ ] Add bounded data and metadata caches with TTL and explicit invalidation; measure directory and large-file behavior before tuning.
- [ ] Keep token and signed URL values out of logs; require HTTPS for API-provided transfer URLs and validate certificates.
- [ ] Validate remote names and path handling, enforce mount permissions, and document token storage and recovery.
- [ ] Add useful request correlation and latency diagnostics without exposing file contents or credentials.

Verification: repeated reads avoid unnecessary transfers, mutations invalidate stale data, and security checks pass for malformed inputs.

## 7. Package and Release

- [ ] Run unit, mocked API, and FUSE integration tests in CI; document the Linux FUSE prerequisites.
- [ ] Publish reproducible release builds and distro installation/uninstallation guidance.
- [ ] Document supported operations, known limitations, troubleshooting, and a final real-account smoke-test procedure.

Verification: a fresh supported Linux machine can install, authenticate, mount, perform supported operations, and unmount successfully.
