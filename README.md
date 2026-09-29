# naver-mybox-linux

Unofficial NAVER MYBOX Adapter for Linux.

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
Writes are buffered in memory and uploaded on flush or close. Editing an existing
file first downloads its entire contents, so very large files may be slow or fail
if the download times out. Path-based truncation uploads immediately. Check for
upload errors when closing files. An open file can still be used after unlink
until its last handle closes; deleting it does not upload buffered edits again.

Files use fixed `0644` permissions and directories use `0755`. Other creation
modes and changes to permissions, ownership, or timestamps are unsupported and
return an error. Before editing and uploading, the mount checks remote file size
and modification time to reject detectable concurrent changes. This is not an
atomic compare-and-swap: another client can still change the file between the
last check and the upload. Avoid concurrent edits from other clients.

Moving an item to another folder while changing its name requires two MYBOX
requests. If the rename fails after the move, the item remains in the destination
folder under its old name. Moving into root requires a nonempty root listing to
obtain MYBOX's root folder ID.

## Debug Logs

Run `cargo run -- mount /path/to/mountpoint` to see debug logs for file downloads,
including the requested byte range, response status, and content headers.
Body-read errors report their I/O kind without logging signed URLs. File reads
request only the bytes needed by FUSE.
Debug logs are enabled by default in debug builds and are unavailable in release builds.
Set `RUST_LOG=info` to reduce logging in a debug build.

Implementation plans are maintained in [`PLAN.md`](./PLAN.md). See the
[`docs/`](./docs/) directory for the [architecture](./docs/architecture.md)
and [workflow notes](./docs/workflows.md).
