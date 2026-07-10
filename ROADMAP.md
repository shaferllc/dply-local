# Roadmap

Candidate work beyond 0.2.x, grouped by theme. Each item notes *why* — most trace
to a real gap or a bottleneck measured while building the current release.

## Distribution & trust

### 1. Signed & notarized releases
Ship the DMG with a Developer ID signature and Apple notarization. Today the app
is ad-hoc signed, so Gatekeeper blocks it on first open and setup has to use a
per-run authorization sheet. With a real identity, setup can move to
`SMAppService` — a resident privileged helper approved once in System Settings —
instead of prompting on every `dpl setup`. **Blocks #2.**

### 2. Self-updating app (Sparkle)
The app checks GitHub Releases and updates in place. The release CI already
builds the DMG on every tag; add an `appcast.xml` the workflow publishes, and wire
Sparkle into the bundle. No more "download the new DMG" — it just updates.

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

### 7. Committable team config (`dpl.toml`)
A per-repo file capturing PHP/Node versions, extensions, services, and Xdebug so a
teammate runs one `dpl up` and gets an identical environment. Turns "works on my
machine" setup into a checked-in artifact; complements the existing `dpl parity`.

### 8. Per-branch database snapshots
One command to dump/restore a site's databases, tied to the current git branch, so
switching branches switches data. Removes the "migrate down / reseed" dance when
hopping between feature branches.

## Performance

### 9. Cold-start elimination: opcache preload + pool warming
First-request latency was 1–1.8s in testing (fresh php-fpm master, cold opcache) vs
~100–200ms warm. Enable `opcache.preload` per site and pre-warm one worker on
reconcile so the first hit after a restart is fast. Target sub-100ms cold serve.

### 10. Push-based tailing + incremental reconcile
Two idle-cost wins at scale (100+ sites):
- Replace the System panel's 1.5s log poll with a `DispatchSource` file watcher —
  instant updates, zero CPU when idle.
- Make the daemon's reconcile incremental: touch only changed sites instead of
  rebuilding every route on each config change. Also worth: a pooled/persistent
  FastCGI connection to php-fpm instead of one socket per request.

---

*Sizing, sequencing, and cuts TBD. #1 unblocks #2; #3 and #9 are the biggest
reliability/perf levers; #4 is the largest single feature.*
