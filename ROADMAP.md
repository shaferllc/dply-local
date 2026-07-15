# Roadmap

Candidate work beyond 0.2.x, grouped by theme. Each item notes *why* — most trace
to a real gap or a bottleneck measured while building the current release.

## Distribution & trust

### 1. Signed & notarized releases
Ship the DMG with a Developer ID signature and Apple notarization. Today the app
is ad-hoc signed, so Gatekeeper blocks it on first open and setup has to use a
per-run authorization sheet. With a real identity, setup can move to
`SMAppService` — a resident privileged helper approved once in System Settings —
instead of prompting on every `dpl setup`. (No longer blocks #2 — Sparkle shipped on EdDSA.)

### 2. ✅ Self-updating app (Sparkle) — shipped
Sparkle 2 checks `releases/latest/download/appcast.xml` daily and updates in
place; CI signs every DMG with the project's EdDSA key and publishes the
appcast as a release asset. Shipped *without* #1: EdDSA authenticates updates
for the ad-hoc-signed app, so #1's remaining value is the first-install
Gatekeeper experience (and SMAppService setup), not updates.

### 3. Bundled, pinned PHP runtimes
Ship dply-built PHP binaries per version instead of resolving the user's Homebrew
`php-fpm`. This kills an entire bug class: the 0.2.0 pgsql segfault came from the
user's `swoole` extension calling Homebrew's GSSAPI `libpq` in a forked worker —
things dply never chose and can't test against. Own the runtime → reproducible
across machines, and `PGGSSENCMODE` becomes belt-and-braces instead of load-bearing.

## Observability

### 4. Notifications engine
Native push for worker failures, N+1 query bursts, slow routes, and long-running
service ops finishing. This is the hard one — it's not a panel, it's detection:
per-route response-time baselining, a query-shape watcher that flags the same
query run in a loop, and a delivery layer. Highest signal, most work.

### 5. Web Tinker & query inspector
A browser REPL into a site (`artisan tinker`) plus a live query inspector, served
same-origin — so debugging never leaves the browser. Builds on the `dpl tinker`
CLI already shipped; the query inspector rides the same dumps/debug bridge.

### 6. Richer debug capture
Expand the dumps receiver into the full picture the app can already sink: queries,
jobs, views, mail, cache, events, and outbound HTTP — each a filterable tab keyed
by request, like the mockups. The transport (`:9912`) exists; this is capture +
presentation.

## Workflow

### 7. ✅ Committable team config (`dpl.toml`) — shipped
`dpl up` applies a repo's committed `dpl.toml` declaratively (PHP pin, HTTPS,
runtime, Xdebug, profiler, preload, branch database, required services — checked,
not auto-created); `dpl up --save` captures the current site into the file. Node
is deliberately omitted: `.nvmrc` already travels with the repo. Extensions and
service auto-provisioning remain as follow-ups.

### 8. ✅ Per-branch databases — shipped in 0.4.0, beyond the original spec
Landed as *branch-aware databases* (`dpl db attach`): instead of dump/restore on
demand, the base database follows `git checkout` automatically — a daemon watcher
swaps branch data via catalog renames (~90ms), cloning only on a branch's first
visit. Postgres-only for now; MySQL (dump/restore path) and parked-DB GC for
deleted branches remain as follow-ups.

## Performance

### 9. ✅ Cold-start elimination — shipped in 0.3.0
`dpl preload` (per-site `opcache.preload` on a dedicated master) plus the warm-up
FastCGI hit on reconcile.

### 10. Push-based tailing + incremental reconcile — first half shipped in 0.3.0
- ~~Replace the System panel's 1.5s log poll with a `DispatchSource` file
  watcher~~ — shipped (event-driven tailing).
- Still open: make the daemon's reconcile incremental — touch only changed sites
  instead of rebuilding every route on each config change. Also worth: a
  pooled/persistent FastCGI connection to php-fpm instead of one socket per
  request. (0.4.0 also cached per-site repo metadata across reconciles, so the
  `dpl sites` read path no longer crawls the filesystem.)

---

*Sizing, sequencing, and cuts TBD. #1 unblocks #2; #3 and #9 are the biggest
reliability/perf levers; #4 is the largest single feature.*
