# Dply Local — SwiftUI site manager

A native macOS app for managing your dply sites (and, as the daemon phases land,
your local `.test` sites). It is a **thin front-end over the `dpl` CLI**: every
action shells out to `dpl … --json` and renders the result, so the GUI and CLI
never drift.

## What it does today

- Three-column browser: **Edge Sites**, **Server Sites**, **Servers**, **Account**.
- Per-site detail with live data: overview, deployments, environment, domains,
  firewall.
- Actions: **Deploy**, **Purge cache**, **Logs** (viewer sheet), **Open** live URL.
- **Log in / out** of dply (device flow) from the Account tab — writes the shared
  `~/.dply/config.json`, so the CLI and app share one login.
- Host + auth indicator in the sidebar footer; `--host` targeting via Settings.

Local `.test` site management drops into the same sidebar once daemon Phases 2–4
exist.

## Build & run

Requires the `dpl` binary (auto-detected from `../target/{debug,release}/dpl` or
`$PATH`; override in Settings).

```bash
# run from source
swift run

# or build a real .app on your Desktop
./make-app.sh            # → ~/Desktop/DplyLocal.app
./make-app.sh --install  # → /Applications/DplyLocal.app and launch
```

## Architecture

| File | Role |
|------|------|
| `App.swift` | `@main` app + `NSApplicationDelegate` (regular-app activation). |
| `DplyCLI.swift` | Resolves and runs the `dpl` binary; decodes `--json` output. |
| `JSONValue.swift` | Tolerant JSON model + `Row` with fallback-name/dotted-path field access. |
| `Store.swift` | `@MainActor ObservableObject` app state + actions. |
| `Views/*` | Sidebar, list, detail panes, logs/login sheets, settings. |

**Threading rule:** the `Store.background(_:)` helper runs the CLI call off the
main actor and returns the value; **all `@Published` mutations happen on the
main actor** in the caller. (Mutating published state off-main crashes SwiftUI's
menu updates — do not reintroduce that.)
