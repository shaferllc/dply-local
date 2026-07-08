# dply-local (`dpl`)

A Rust local PHP development environment in the mold of
[yerd](https://github.com/forjedio/yerd) — serve `.test` sites over HTTP/HTTPS,
run multiple PHP versions, manage local databases — that **also speaks to the
[dply](https://dply.app) deployment platform**, so you can push and manage a
local project on dply from the same cockpit.

Rootless for daily use; elevates once at setup. macOS + Linux.

## Workspace layout

| Crate        | Binary       | Role |
|--------------|--------------|------|
| `dpl-core`   | —            | Shared vocabulary: paths, IPC protocol, local config, cross-platform seams. |
| `dpl-dply`   | —            | Full Rust client for the dply v1 API (parity with the PHP `dply` CLI). |
| `dpl`        | `dpl`        | The CLI. Local commands talk to `dpld`; `dpl dply …` talks to dply directly. |
| `dpld`       | `dpld`       | Per-user daemon: proxy + DNS + PHP-FPM + services (built out over phases). |
| `dpl-helper` | `dpl-helper` | One-shot privileged helper: CA trust, resolver config, low-port binding. |

## Status

**Phase 0 (foundation) and Phase 1 (dply integration) are done.**

- `dpl ping` / `dpl status` / `dpl version` round-trip to the `dpld` daemon over
  a per-user unix socket (length-prefixed JSON, versioned handshake).
- `dpl dply …` mirrors the whole dply CLI surface — `edge:*`, `sites:*`,
  `site:env`, `servers:*`, `insights:*`, `imports:*`, `operator:*` — with
  `--host` and `--json` on every command.
- `dpl login` uses the dply device-authorization flow and stores the token in
  the **shared `~/.dply/config.json`**, so a login here is honoured by the
  existing PHP `dply` CLI and vice-versa.

Later phases (roadmap): local `.test` HTTP proxy → PHP-FPM, DNS + `dpl-helper`,
HTTPS via a local CA, multi-PHP management, database services, mail capture,
Cloudflare tunnels, and a **site-management GUI**.

## Build & run

```bash
cargo build
cargo test -p dpl-dply          # client + config-store tests

# start the daemon (foreground for now)
./target/debug/dpld &

./target/debug/dpl ping                 # -> pong
./target/debug/dpl login                # device-flow login to dply
./target/debug/dpl dply edge:sites      # list your edge sites
./target/debug/dpl dply sites:show my-site
./target/debug/dpl dply edge:logs my-site --tail
```

Target a non-default dply host per invocation with `--host`, e.g.
`--host=https://dplyi.test` for a local instance.

## License

MIT.
