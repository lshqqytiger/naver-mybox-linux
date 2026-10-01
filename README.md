# naver-mybox-linux

Unofficial NAVER MYBOX Adapter for Linux.

NAVER and MYBOX are trademarks of NAVER Corporation. This independent project
is not affiliated with, sponsored by, or endorsed by NAVER Corporation.

## Disclaimer

This software is provided "as is", without warranty, under the terms of the
[LICENSE](./LICENSE). It can modify or delete files in your MYBOX account.
Back up important data and test with noncritical files before relying on it.

## Setup

Install Rust/Cargo and the FUSE 3 runtime (`fuse3` on Debian/Ubuntu), and ensure
`/dev/fuse` is available. Build and install:

```bash
cargo build --release --bins
sudo install -m 0755 target/release/myboxfs /usr/local/bin/myboxfs
sudo install -m 0755 target/release/mount-myboxfs /sbin/mount.myboxfs
```

The mount helper is only needed for `/etc/fstab` support.

## Login

Create a personal access token in MYBOX web settings under **Account and personal access token management**, then run:

```bash
myboxfs login
```

Log in as your normal user, not root. Use `myboxfs health-check` to verify the
saved token. Never put the token in shell arguments or `/etc/fstab`.
Token directories must be owned by your user with mode `0700`; older installations
may need `chmod 700 ~/.config/myboxfs` before saving a replacement token.

## Mount

```bash
mkdir -p ~/MYBOX
myboxfs mount ~/MYBOX
```

Keep the command running. Browse and edit files normally, then unmount from
another terminal:

```bash
myboxfs unmount ~/MYBOX
```

Files and folders can be created, edited, renamed, moved, and deleted. Deletion
moves items to MYBOX trash. See [behavior and limitations](docs/workflows.md)
before using important data.

## /etc/fstab

After login and helper installation, create a mountpoint owned by your user.
Replace `alice` and its group below with your local account:

```bash
sudo install -d -m 0755 -o alice -g alice /mnt/mybox
```

Add this entry to `/etc/fstab`, replacing `alice` with the local login name:

```fstab
alice /mnt/mybox myboxfs defaults,_netdev,noauto 0 0
```

```bash
sudo mount /mnt/mybox
myboxfs unmount /mnt/mybox
```

For custom token paths, boot mounting, and mount options, see the
[fstab guide](docs/workflows.md#fstab-mounting).

## Documentation

- [Workflows, limitations, and troubleshooting](docs/workflows.md)
- [Architecture and implementation](docs/architecture.md)
