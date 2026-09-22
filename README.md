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

Implementation plans are maintained in [`PLAN.md`](./PLAN.md).
