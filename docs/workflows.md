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

## Browsing and Reading

The root directory is represented by a synthetic inode. Its first listing
calls the root resources endpoint; a nested directory is listed on first
access through its resource ID. Listings refresh after five seconds,
preserving inodes for unchanged resource IDs. Remote changes can remain
invisible until the next refresh.

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
file. A failed upload leaves the staged file available for a later attempt
during the same mount. A path-based truncate uploads before reporting success;
pending edits do not survive process exit. Before staging and uploading, the
mount checks remote size and modification time when available. This check is
best-effort, not an atomic conditional write. Mutations are not automatically
retried because their outcome may be ambiguous after a network failure.

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
enable `allow_other`.

## Scope and Limitations

The current implementation does not provide full POSIX semantics, special
files, advanced ACLs, offline synchronization, atomic conflict detection,
cross-mount cache invalidation, or bounded retained directory metadata. Large
edits require enough local temporary disk space. Live-account behavior and
the API response contract still require the smoke tests tracked in
[`PLAN.md`](../PLAN.md).
