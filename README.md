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

**Phases 0–6 are in.** dply-local is a working local dev environment.

- **dply integration** — `dpl dply …` mirrors the whole dply CLI surface
  (`edge:*`, `sites:*`, `site:env`, `servers:*`, `insights:*`, `imports:*`,
  `operator:*`) with `--host`/`--json`; `dpl login` uses the device-auth flow and
  shares `~/.dply/config.json` with the PHP `dply` CLI both ways.
- **Local serving** — `.test` sites over an HTTP/HTTPS proxy to per-site php-fpm
  pools, wildcard DNS responder, and a local CA for trusted HTTPS. `:80`/`:443`
  are handed to `dpld` by a launchd-activated socket (installed via
  `dpl setup`); the daemon runs as the login user, never root.
- **PHP** — multi-version management and per-site pinning, an extension manager,
  and per-site Xdebug modes (one php-fpm pool per mode).
- **Services** — database engines, mail capture (SMTP sink with per-site
  mailboxes + a MIME/HTML viewer), a debug-dump receiver, and Cloudflare tunnels.
- **Tooling** — `dpl doctor` health checks with one-click fixes, `dpl parity`,
  and Valet import.
- **GUI** — `DplyLocal.app` (SwiftUI), a native front-end that drives the same
  `dpl` binary the terminal uses; privileged setup runs in-app behind the macOS
  authorization sheet.

Not yet: a Developer ID-signed bundle (so setup could use `SMAppService` for a
resident helper instead of a per-invocation authorization prompt).

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

Proprietary. Licensed builds are distributed through Chandlery.
