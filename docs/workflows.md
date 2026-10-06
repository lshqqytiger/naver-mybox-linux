# Workflows and Behavior

## Authentication and Mounting

1. Run `myboxfs login` and provide a MYBOX personal access token when
   prompted.
2. The CLI validates the token with the MYBOX storage endpoint, then saves
   it to `$XDG_CONFIG_HOME/myboxfs/token`, or `~/.config/myboxfs/token` when
   `XDG_CONFIG_HOME` is unset or empty.
3. Run `myboxfs health-check` to validate the saved token again.
4. Run `myboxfs mount MOUNTPOINT`. The CLI loads the saved token and starts
   the FUSE filesystem. Run `myboxfs unmount MOUNTPOINT` to invoke
   `fusermount3` or `fusermount`.

Saved token files are created privately with mode `0600`, synced, and atomically
replaced in an owner-only directory. Saving rejects symlink token files and
symlink or non-private token directories. For an existing token directory from
an older installation, set its permissions to `0700` before logging in again
(for example, `chmod 700 ~/.config/myboxfs`). Protect your home directory and never put
tokens in mount options, shell arguments, or `/etc/fstab`. If a token is lost
or exposed, revoke it in MYBOX settings and run `myboxfs login` again. Only
mount on directories you trust and unmount before removing local temporary
storage. Files use the mounting user's UID/GID; other users cannot access the
mount by default.

## Fstab Mounting

Build and install the helper as described in the [setup guide](../README.md#setup).
The installed name must be `mount.myboxfs` in the system mount-helper directory,
usually `/sbin` or its merged `/usr/sbin` equivalent. Run `myboxfs login` as the
intended local user and give that user ownership of the mountpoint.

```fstab
alice /mnt/mybox myboxfs defaults,_netdev,noauto 0 0
```

The source field selects the local user whose MYBOX account is mounted. The
helper drops root privileges and sets that user's supplementary groups before
reading credentials or starting FUSE. It finds the user's home in the system
account database and reads `.config/myboxfs/token` beneath it, never using
root's `HOME` or `XDG_CONFIG_HOME`. If login used a custom `XDG_CONFIG_HOME`, add
`token_file=/absolute/path/to/myboxfs/token` to the options. The selected user
must be able to read that file. Never put a token itself in fstab.

The helper runs in the background and returns only after FUSE has mounted;
startup errors produce a nonzero exit status. Use `-o foreground` when invoking
`mount.myboxfs` directly to keep it attached to the terminal with stderr logging.
Background mounts send tracing events and terminal session errors to `/dev/log`
with the `myboxfs` identifier (for example, `journalctl -t myboxfs` on journald
systems). The helper fails startup if it cannot open this logging destination;
use a supervised foreground mount on systems without it. `RUST_LOG` controls
logging in both modes. `-f` is a dry run,
not foreground mode. Add `user` to fstab to permit the usual non-root
`mount /mnt/mybox` workflow where supported by the distribution's mount utility.
For boot mounting, remove `noauto` and consider `nofail`; `_netdev` orders the
mount after networking but does not guarantee MYBOX connectivity. Encrypted
home directories must be unlocked before their token files can be read.

Supported FUSE options are `ro`/`rw`, `exec`/`noexec`, `atime`/`noatime`,
`sync`/`async`, `dirsync`, `allow_other`, and `allow_root`. Opposing flags use
the last supplied value. `default_permissions`, `nosuid`, and `nodev` are
always enforced; ownership cannot be overridden using `uid` or `gid`.
`allow_other` grants other users access subject to the fixed file permissions;
`allow_root` grants access to root and the owner. Both require
`user_allow_other` in `/etc/fuse.conf` for an unprivileged mount. Fstab options
such as `defaults`, `_netdev`, `noauto`, `user`, `nofail`, and `x-systemd.*`
are accepted as mount-manager metadata. Unknown or unsafe options are rejected,
even with `-s`, rather than silently ignored.

Verify an installed helper with `sudo mount /mnt/mybox`, `findmnt /mnt/mybox`,
and read/write tests on noncritical files, then unmount. To uninstall, unmount
first, remove the fstab entry, and remove `/sbin/mount.myboxfs` and
`/usr/local/bin/myboxfs`. Real-account and boot-mount verification still require
MYBOX credentials and a FUSE-capable machine.

## Browsing and Reading

The root directory is represented by a synthetic inode. Its first listing
calls the root resources endpoint; a nested directory is listed on first
access through its resource ID. Listings refresh after five seconds,
preserving inodes for unchanged resource IDs. Remote changes can remain
invisible until the next refresh.

Remotely removed or moved entries and their loaded descendants are detached
from directory listings. Clean detached nodes are reclaimed when open and lookup
references drain; dirty nodes retain their staged data for recovery.

File reads request the requested byte range through MYBOX's download URL.
If the server returns a full `200 OK` response instead of partial content,
the client skips to the requested offset before reading the requested number
of bytes. Clean ranges are cached for two seconds within a 4 MiB budget;
edits invalidate the affected ranges. Debug builds log download response
headers and sanitized body-read failures; release builds omit those debug
diagnostics. Read-only requests retry transient 429, 5xx, and timeout failures
up to twice. Transfer URLs must use HTTPS with valid TLS certificates.

## Creating and Editing Files

Creating a file uploads an empty file first. Subsequent writes stage file
contents in a private temporary file. Editing an existing file downloads it
in 1 MiB chunks and refuses to stage it if any chunk is incomplete. Truncating
to zero starts with an empty temporary file; other truncations stage the
existing contents before resizing.

`flush`, `fsync`, and file release upload the staged contents with overwrite
enabled, streaming the staged file instead of copying it into memory. A
successful upload replaces the in-memory remote entry and drops the temporary
file. A failed, uncommitted upload leaves the staged file available for a later
attempt during the same mount. If the upload succeeds but metadata reconciliation
fails, the mount records a committed pending upload rather than retrying it.
Before accepting further edits, it verifies the resource ID in the parent listing,
stable metadata, and the complete remote contents against the staged bytes.
Missing or changed resource identity, listing failures, and byte mismatches fail
closed while retaining staged data. If an overwrite changes the resource ID and
the API supplies no authoritative commit identity, automatic reconciliation is
not possible. Pending reconciliation also blocks unlink and rename of that file.
A path-based truncate uploads before reporting success;
pending edits do not survive process exit. Before staging and uploading, the
mount checks remote size and modification time when available. This check is
best-effort, not an atomic conditional write. Upload preflight also verifies that
the cached filename in the expected parent still identifies the staged resource,
rejecting detected remote renames, moves, and filename reuse. Mutations are not automatically
retried because their outcome may be ambiguous after a network failure.

Check errors from `flush` and `fsync`: failed closes may not be reported by
every application, and unflushed data does not survive unmount or process exit.
An open file can still be used after unlink until its last handle closes;
deleting it does not upload buffered edits again. Avoid concurrent edits from
other clients or handles. Another client can change a file between the version
check and upload, and when MYBOX omits modification time only size is compared.

## Directories and Mutations

| Operation                 | Current behavior                                                                                                       |
| ------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `mkdir`                   | Creates a remote folder and adds it to the parent inode table.                                                         |
| `rmdir`                   | Removes only an empty folder; checks both the loaded children and the remote listing.                                  |
| `unlink`                  | Deletes a remote file and removes its inode from the in-memory parent listing. MYBOX deletion moves the item to trash. |
| Rename within a directory | Renames the remote resource, provided the destination name does not exist.                                             |
| Move to another directory | Moves the remote resource and updates its in-memory parent.                                                            |
| Move and rename together  | Performs a move followed by a rename. If the rename fails, the item remains moved under its old name.                  |

Replacement of an existing destination is not supported. Moving an item into
the root requires the API client to infer MYBOX's root folder ID from a
nonempty root listing; this fails if that listing is empty. Names must be
valid UTF-8 and may not be empty, `.` or `..`, or contain `/` or NUL. Remote
resource IDs are encoded as URL path segments. The mount uses fixed file and
directory modes (`0644` and `0755`) and FUSE default permissions; it does not
enable `allow_other` by default. The fstab helper can enable it explicitly.
Directory creation accepts the requested mode and umask but exposes the fixed
`0755` directory mode. Other file creation modes and changes to permissions,
ownership, or timestamps are unsupported and return an error.

Extended attribute probes return an empty list; reading any named attribute
(including `security.capability`) returns `ENODATA`. Unknown inodes return
`ENOENT`. Setting or removing extended attributes remains unsupported; no
extended attributes are stored locally or synchronized with MYBOX.

## Network Failures

Read-only requests retry up to twice after HTTP 429, 5xx, or a timeout, with
bounded backoff. Uploads, deletes, moves, and renames are not retried because a
failed response may follow a successful remote mutation. A 10-second connection
timeout and five-minute total request timeout apply. Known HTTP failures are
mapped to filesystem errors (for example, 404 to `ENOENT`, 403 to `EACCES`,
429 to `EAGAIN`); other failures may return `EIO`.

## Debug Logs

Run `cargo run -- mount /path/to/mountpoint` to see debug logs for file downloads,
including the requested byte range, response status, and content headers.
Body-read errors report their I/O kind without logging signed URLs. File reads
request only the bytes needed by FUSE. Request logs include an ID, operation,
HTTP status, attempt, and elapsed time, but never a token, signed URL, or file
contents. Transfer URLs must use HTTPS; TLS certificates are verified by the
HTTP client and redirects cannot downgrade to HTTP.

Debug logs are enabled by default in debug builds and are unavailable in release
builds. Set `RUST_LOG=info` to reduce logging in a debug build.

## Scope and Limitations

The current implementation does not provide full POSIX semantics, special
files, advanced ACLs, offline synchronization, atomic conflict detection,
cross-mount cache invalidation, or bounded retained directory metadata. Large
edits require enough local temporary disk space.
