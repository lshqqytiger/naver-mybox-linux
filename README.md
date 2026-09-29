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

## Read-only Mount

```bash
myboxfs mount /path/to/mountpoint
```

The initial mount supports directory browsing, file attributes, and file reads. It is mounted read-only.

## Debug Logs

Run `cargo run -- mount /path/to/mountpoint` to see debug logs for file downloads,
including the requested byte range, response status, content headers, and underlying
body-read errors. File reads request only the bytes needed by FUSE.
Debug logs are enabled by default in debug builds and are unavailable in release builds.
Set `RUST_LOG=info` to reduce logging in a debug build.

Implementation plans are maintained in [`PLAN.md`](./PLAN.md).
