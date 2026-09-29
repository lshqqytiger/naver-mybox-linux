# naver-mybox-linux

Unofficial NAVER MYBOX Adapter for Linux.

NAVER and MYBOX are trademarks of NAVER Corporation. This independent project
is not affiliated with, sponsored by, or endorsed by NAVER Corporation.

## Disclaimer

This software is provided "as is", without warranty, under the terms of the
[LICENSE](./LICENSE). It can modify or delete files in your MYBOX account.
Back up important data and test with noncritical files before relying on it.

## Login

Create a personal access token in MYBOX web settings under **Account and personal access token management**, then run:

```bash
myboxfs login
```

The token is validated against the MYBOX storage API before it is saved to
`$XDG_CONFIG_HOME/myboxfs/token` (or `~/.config/myboxfs/token`) with owner-only permissions.
Use `myboxfs health-check` to verify the saved token.

## Mount

```bash
myboxfs mount /path/to/mountpoint
```

The mount supports directory browsing, file reads, file creation and modification,
empty-folder creation and removal, and file/folder renaming and moving. Deletion
moves items to MYBOX trash. Renaming does not replace an existing destination.
Writes are staged in private temporary files and streamed to MYBOX on `flush`,
`fsync`, or close. Editing an existing file downloads its contents in 1 MiB
chunks first, so large edits need enough local temporary disk space and may take
time. A zero-length truncation does not download the old contents. Path-based
truncation uploads immediately. A failed upload retains the staged data until a
later successful flush or the mount exits; check errors from `flush`/`fsync`.
A failed close may not be reported by every application, and unflushed data does
not survive process exit or unmount. An open file can still be used after unlink
until its last handle closes; deleting it does not upload buffered edits again.

Files use fixed `0644` permissions and directories use `0755`. Other creation
modes and changes to permissions, ownership, or timestamps are unsupported and
return an error. Before editing and uploading, the mount checks remote file size
and modification time to reject detectable concurrent changes. This is not an
atomic compare-and-swap: another client can still change the file between the
last check and the upload. When MYBOX omits a modification time, the check can
only compare sizes. Avoid concurrent edits from other clients or handles.

Clean reads use a 4 MiB, two-second cache; directory listings refresh after five
seconds and preserve inodes for unchanged entries. Cached attributes are returned
to the kernel for up to one second. External changes can remain invisible during
those windows; local writes and deletes invalidate cached file reads. Remote
RFC3339 modification timestamps are exposed when available.

Read-only requests retry up to twice after HTTP 429, 5xx, or a timeout, with
bounded backoff. Uploads, deletes, moves, and renames are not retried because a
failed response may follow a successful remote mutation. A 10-second connection
timeout and five-minute total request timeout apply. Known HTTP failures are
mapped to filesystem errors (for example, 404 to `ENOENT`, 403 to `EACCES`,
429 to `EAGAIN`); other failures may return `EIO`.

Moving an item to another folder while changing its name requires two MYBOX
requests. If the rename fails after the move, the item remains in the destination
folder under its old name. Moving into root requires a nonempty root listing to
obtain MYBOX's root folder ID.

## Debug Logs

Run `cargo run -- mount /path/to/mountpoint` to see debug logs for file downloads,
including the requested byte range, response status, and content headers.
Body-read errors report their I/O kind without logging signed URLs. File reads
request only the bytes needed by FUSE.
Request logs include an ID, operation, HTTP status, attempt, and elapsed time,
but never a token, signed URL, or file contents. Transfer URLs must use HTTPS;
TLS certificates are verified by the HTTP client and redirects cannot downgrade
to HTTP.
Debug logs are enabled by default in debug builds and are unavailable in release builds.
Set `RUST_LOG=info` to reduce logging in a debug build.

## Credentials and Access

`myboxfs login` stores the validated token in a mode-`0600` file under
`$XDG_CONFIG_HOME/myboxfs/token` or `~/.config/myboxfs/token`. Protect your
home directory and do not place the token in mount options, shell arguments, or
`/etc/fstab`. If the token is lost or exposed, revoke it in MYBOX settings and
run `myboxfs login` again. The mount uses FUSE's default permissions with the
mounting user's UID/GID and fixed `0644`/`0755` modes; it does not enable
`allow_other`. Only mount on directories you trust and unmount with
`myboxfs unmount /path/to/mountpoint` before removing local temporary storage.

Implementation plans are maintained in [`PLAN.md`](./PLAN.md). See the
[`docs/`](./docs/) directory for the [architecture](./docs/architecture.md)
and [workflow notes](./docs/workflows.md).
