# Design: app opcache preload (cold-start elimination)

Status: proposed. Fourth and final lever after the three shipped perf changes
(warm-worker idle timeout, background worker warm-up, event-driven log tail).

## The problem, measured

dpl's own per-request overhead is already tiny — loopback FastCGI, a microsecond
route lookup. The real latency is the **first request to a cold worker**, which
recompiles the app into opcache before any output. On a Laravel app that's
~1–2s; every request after it is fast off the warm, primed worker.

The three shipped changes attack *keeping* a worker warm:

- `pm.process_idle_timeout = 300s` — a worker (and its primed opcache) survives
  normal dev pauses instead of dying after 30s.
- background warm-up — a new master forks its first worker + runs PHP startup
  off the critical path, but **cannot** prime the app (that needs running it).
- these help the *recurring* cold start, not the *first* one after a daemon or
  master (re)start, which still pays the full app-compile.

Preload closes that last gap: it moves app compilation to **master startup**,
once, into shared memory that every worker inherits and that survives worker
recycling and idle death.

## Mechanism: PHP `opcache.preload`, not compile-on-warm

The earlier warm-up runs a trivial script — it can't compile the app without
executing it, and executing the app has side effects (routes send mail, mutate
the DB). PHP's built-in `opcache.preload` is the right tool:

- Set `opcache.preload = <script>` (+ `opcache.preload_user` when running as
  root; our masters run as the user, so it's optional) in the pool config.
- php-fpm runs that script **once, in the master, at startup**. Classes and
  functions it compiles are linked into shared memory **permanently** for the
  master's life — immune to `pm.max_requests` recycling and idle death, shared
  by every worker.
- The script `require`s the app's autoloader and `opcache_compile_file()`s (or
  otherwise references) the files to preload.

Net effect: the master comes up already holding the compiled app, so the *first*
real request is warm. Combined with the shipped warm-up (forks the first worker)
the very first request is fully hot.

## The core tension: preload is per-master, sites are many-per-master

One master serves every site sharing `(php_bin, xdebug_mode, profile)` — its
opcache SHM is shared across those sites' document roots. But `opcache.preload`
is a **single** script per master. You cannot preload two different apps'
autoloaders into one shared master without collisions and unbounded memory.

Resolution: **a preloaded site gets its own dedicated master**, exactly the
precedent `xdebug` and `profile` already set (a site wanting Xdebug or SPX
forks off its own master today). Preload becomes the fourth master axis.

```
// today:  crates/dpld/src/fpm.rs
pub type MasterKey = (PathBuf, Mode, bool /*profile*/);

// proposed:
pub type MasterKey = (PathBuf, Mode, bool /*profile*/, Option<PathBuf> /*preload script*/);
//   None        → shares the common master (unchanged behavior)
//   Some(script)→ dedicated master with opcache.preload = script
```

`ensure()` / `write_conf()` gain the preload arg; when `Some`, `write_conf`
emits:

```
php_admin_value[opcache.preload] = <script>
php_admin_value[opcache.memory_consumption] = 256   ; preload needs headroom
```

`reconcile()` in `registry.rs` already maps each site to a `MasterKey` and
`retain()`s the live set — it just threads the site's preload script into the
key. No new supervision machinery.

Cost: one extra master per preloaded site. Because preload is **opt-in** and only
worth it on the handful of apps you actively develop, that's a bounded, chosen
cost — the same trade as turning on Xdebug or the profiler.

## The stale-code gotcha (this drives the default)

Preloaded entries are compiled at master start and are **permanent for the
master's life** — `opcache.validate_timestamps` does *not* re-check them. Preload
a file you're actively editing and your edits won't take effect until the master
restarts.

Therefore the default preload target is **`vendor/` (framework + dependencies),
not `app/`**. Vendor code is large, slow to compile, and rarely changes — ideal
to preload. App code changes every save — never preload it by default. This
inverts the naive "preload everything" and is what makes preload safe for a live
dev loop.

Freshness for vendor: watch each preloaded site's `composer.lock`; on change,
drop and respawn that master so preload rebuilds against the new vendor tree.
The daemon already reconciles on config changes — add a `composer.lock` mtime to
the reconcile trigger for preloaded sites. No manual `dpl reload` needed.

## Config & command surface

Per-site opt-in on the existing `Link` struct (`crates/dpl-core/src/config.rs`,
persisted in `~/.dpl/config.toml`):

```rust
/// Preload script for this site's php-fpm master (opcache.preload). A preloaded
/// site gets its own master. Relative to the project root; None = no preload.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub preload: Option<PathBuf>,
```

CLI (mirrors `dpl profile` / `dpl xdebug`):

- `dpl preload generate [site]` — scaffold a starter `dpl-preload.php` at the
  project root that `require`s `vendor/autoload.php` and compiles the framework +
  a bounded vendor subset, with app/ commented out and the stale-code caveat in
  a header comment. The user owns and tunes the file.
- `dpl preload on <site> [--script <path>]` — set `Link.preload` (defaults to
  `dpl-preload.php`; errors if the script is missing, pointing at `generate`).
- `dpl preload off <site>` — clear it; the site folds back into the shared master.
- `dpl preload status` — per-site table: on/off, script path, resolved master,
  and (from the master log) whether the last preload succeeded or warned.

GUI: a Preload toggle in `SiteDetailView` alongside the existing Xdebug/Profile
toggles, gated on a generated script existing (offer "Generate…" when it doesn't).

## Why a user-owned script, not dpl guessing

`opcache.preload` is finicky: a preloaded class whose parent/interface isn't yet
loaded warns or fails at startup, and over-preloading blows
`opcache.memory_consumption`. Rather than have dpl guess a safe, complete file
list across arbitrary apps (and silently mis-preload), dpl **scaffolds** a
sensible starter and lets the user opt in and adjust. `generate` encodes the good
defaults (vendor-first, app excluded, bounded); the user keeps control of the
finicky part. This matches how preload works in the real world (Laravel/Composer
preload scripts are app-authored).

## Interactions

- **warm-up (shipped):** still fires; preload primed the SHM at master start, the
  warm-up forks the first worker — together the first real request is fully hot.
- **idle timeout (shipped):** irrelevant to preloaded entries (they live in the
  master, not the worker) but still keeps app-runtime state warm.
- **xdebug / profile:** orthogonal axes on the same key; a site that is both
  preloaded and profiled gets one master carrying both. The key already
  distinguishes them.
- **Octane runtimes:** N/A — Octane sites don't use php-fpm masters; they keep
  the framework booted in a long-lived process already (their own cold-start story).

## Risks & non-goals

- **Memory:** each preloaded site = one dedicated master + 256MB opcache SHM.
  Opt-in and scoped to chosen sites; `dpl preload status` should surface the
  master count so it's visible. Non-goal: preloading every site automatically.
- **Preload failures:** a bad script warns at master start and the master may
  refuse to serve. `status` must read the master log and report it clearly, and
  `on` should refuse a missing script up front.
- **Not a substitute** for the shipped changes — it's the top of the same stack,
  for the sites where the first-request latency actually matters.

## Phased implementation

1. **Core plumbing:** add `Option<PathBuf>` to `MasterKey`; thread through
   `ensure`, `write_conf` (emit the two `php_admin_value`s), and `reconcile`'s
   key construction. Behavior identical when `None`. Tests: a `Some` key writes
   the preload directives and forks a distinct master.
2. **Config + CLI:** `Link.preload` field; `dpl preload on/off/status`.
3. **Scaffold:** `dpl preload generate` writing `dpl-preload.php`.
4. **Freshness:** `composer.lock` mtime in the reconcile trigger for preloaded
   sites → auto-respawn.
5. **GUI:** `SiteDetailView` toggle + generate affordance.
6. **Docs/doctor:** `dpl preload status` master-log parsing; a doctor note when a
   preload master last failed.

Phases 1–2 deliver the win; 3–6 are polish and can land incrementally.
