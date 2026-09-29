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
access through its resource ID. The resulting entries are retained in the
in-memory inode table for the lifetime of the mount.

File reads request the requested byte range through MYBOX's download URL.
If the server returns a full `200 OK` response instead of partial content,
the client skips to the requested offset before reading the requested number
of bytes. Debug builds log download response headers and body-read error
causes; release builds omit those debug diagnostics.

## Creating and Editing Files

Creating a file uploads an empty file first. Subsequent writes stage file
contents in memory. Editing an existing file first downloads the entire file
and refuses to stage it if that download is incomplete. Truncating to zero
starts with an empty staged buffer; other truncations stage the existing
contents before resizing.

`flush`, `fsync`, and file release upload the staged contents with overwrite
enabled. A successful upload replaces the in-memory remote entry and clears
the staged buffer. A failed upload returns an I/O error and leaves the staged
buffer available for a later attempt during the same mount. Buffering is
unbounded in memory, and no edits survive process exit before a successful
upload.

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
valid UTF-8 and may not be empty, `.` or `..`, or contain `/`.

## Scope and Limitations

The current implementation does not provide full POSIX semantics, special
files, advanced ACLs, offline synchronization, bounded write storage,
cross-mount cache invalidation, or automatic retry/backoff. Large-file edits
require a full in-memory copy. Live-account behavior and the API response
contract still require the smoke tests tracked in [`PLAN.md`](../PLAN.md).
