//! The `dpl` command tree.
//!
//! Two families live side by side:
//! - **local** control verbs (`ping`/`status`/`version`) that round-trip to
//!   the `dpld` daemon;
//! - the **`dply`** subtree, a colon-named mirror of the PHP `dply` CLI
//!   (`dpl dply edge:sites`, `dpl dply sites:show`, …) for full API parity.
//!
//! Global flags (`--host`, `--json`, `--config-dir`) apply to every command;
//! dply commands read them, local commands ignore the dply-specific ones.

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "dpl",
    version,
    about = "Local PHP dev environment + dply client",
    long_about = "dpl serves your .test sites locally (via the dpld daemon) and \
                  talks to your dply deployment platform (dpl dply …)."
)]
pub struct Cli {
    /// Target a specific dply host for this invocation (else the stored default).
    #[arg(long, global = true)]
    pub host: Option<String>,

    /// Emit raw JSON instead of tables (dply commands).
    #[arg(long, global = true)]
    pub json: bool,

    /// Override $HOME for config/socket resolution (testing / sandboxes).
    #[arg(long = "config-dir", global = true)]
    pub config_dir: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Check the daemon is alive.
    Ping,
    /// Show daemon status.
    Status,
    /// Print daemon and CLI versions.
    Version,

    /// List the local `.test` sites the daemon is serving.
    Sites,
    /// Park a directory: each subdirectory becomes a `<name>.test` site.
    Park {
        /// Directory to park (default: current directory).
        path: Option<String>,
    },
    /// Stop parking a directory.
    Unpark {
        /// Directory to unpark (default: current directory).
        path: Option<String>,
    },
    /// Apply this project's committed `dpl.toml` (or write one with --save).
    Up {
        /// Project directory (default: current directory).
        path: Option<String>,
        /// Capture the site's current settings into dpl.toml instead of applying.
        #[arg(long)]
        save: bool,
    },
    /// Link a single project as a `.test` site.
    Link {
        /// Project directory (default: current directory).
        path: Option<String>,
        /// Site name (default: the directory's name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove a linked site.
    Unlink {
        /// Site name (default: current directory's name).
        name: Option<String>,
    },
    /// Serve a linked site over HTTPS (trust lands in Phase 4).
    Secure {
        /// Site name (default: current directory's name).
        name: Option<String>,
    },
    /// Stop serving a linked site over HTTPS.
    Unsecure {
        /// Site name (default: current directory's name).
        name: Option<String>,
    },
    /// Open a local site in the browser.
    Open {
        /// Site name (default: current directory's name).
        name: Option<String>,
    },
    /// Proxy a `.test` host to another local service (Docker, Vite, a port…).
    Proxy {
        /// Host name, e.g. `blog` or `blog.test`.
        name: String,
        /// Target URL, e.g. `http://localhost:3000` (scheme optional).
        target: String,
    },
    /// Remove a reverse proxy.
    Unproxy {
        /// Host name.
        name: String,
    },
    /// List reverse proxies.
    Proxies,
    /// List installed PHP versions, or manage them (install/uninstall/repair).
    Php {
        #[command(subcommand)]
        cmd: Option<PhpCmd>,
    },
    /// Pin a PHP version for a site (or set the default with no site).
    Use {
        /// PHP version, e.g. 8.3.
        version: String,
        /// Site name (default: current directory's name; omit entirely for the global default).
        name: Option<String>,
        /// Set the global default instead of a specific site.
        #[arg(long)]
        default: bool,
    },
    /// Show a local site's request log.
    Logs {
        /// Site name (default: current directory's name).
        name: Option<String>,
        /// Number of trailing lines to show.
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
        /// Follow the log (like `tail -f`).
        #[arg(short, long)]
        follow: bool,
    },
    /// Open a Laravel Tinker REPL for a site, on its pinned PHP version.
    Tinker {
        /// Site name (default: the current directory).
        site: Option<String>,
    },
    /// Share a local site publicly via a Cloudflare quick tunnel.
    Share {
        /// Site name (default: current directory's name).
        name: Option<String>,
    },
    /// Start the dpld daemon.
    Start,
    /// Stop the dpld daemon.
    Stop,
    /// Reload config + reconcile backends without restarting the daemon.
    Reload,
    /// Reload the config from disk and restart backends.
    Restart,
    /// Hard-reset all backends (stop + reap php-fpm/Octane, rebuild) — the fix
    /// when the machine gets into a churned/wedged state.
    Repair,
    /// Show config, socket, and data paths.
    Paths,
    /// Check the local environment and report problems (`--json` for a report).
    Doctor,
    /// Diff a local site against the dply site it deploys to: PHP, extensions,
    /// env key names, branch, and document root. Never reads env values.
    Parity {
        /// Linked site name (default: the site linked to the current directory).
        site: Option<String>,
        /// dply site to compare against, when it's named differently.
        #[arg(long)]
        remote: Option<String>,
    },
    /// One-time privileged setup: trust the CA, route .test, claim :80/:443.
    Setup {
        /// Don't claim :80/:443 (sites stay on :8080/:8443).
        #[arg(long)]
        no_ports: bool,
        /// Install for this user. Needed when setup itself runs as root (the GUI's
        /// authorization prompt), where neither USER nor SUDO_USER names a human.
        #[arg(long, value_name = "NAME")]
        as_user: Option<String>,
    },
    /// Set a linked site's runtime (fpm or an already-installed Octane server).
    Runtime {
        /// Linked site name.
        site: String,
        /// fpm | octane-swoole | octane-roadrunner | octane-frankenphp.
        runtime: String,
    },
    /// Install Laravel Octane in a site and switch it to that server.
    Octane {
        /// Linked site name (a Laravel app).
        site: String,
        /// Server: swoole | roadrunner | frankenphp.
        #[arg(long, default_value = "frankenphp")]
        server: String,
    },
    /// SPX flame-graph profiler per site. `dpl profile` with no subcommand shows
    /// each site's status; `on`/`off` toggle it; `open` launches the flame graphs.
    Profile {
        #[command(subcommand)]
        command: Option<ProfileCmd>,
    },
    /// Opcache preload per site — compile a site's (typically vendor) code into
    /// shared memory once at php-fpm start, so the first request is warm. `dpl
    /// preload` shows status; `generate` scaffolds a script; `on`/`off` toggle it.
    Preload {
        #[command(subcommand)]
        command: Option<PreloadCmd>,
    },
    /// Per-project Node version, via fnm/nvm. `dpl node` shows each site's pinned
    /// version; `use` writes its `.nvmrc`; `install` installs a version.
    Node {
        #[command(subcommand)]
        command: Option<NodeCmd>,
    },
    /// Control Xdebug per site: step debugging, profiling, tracing.
    Xdebug {
        #[command(subcommand)]
        command: Option<XdebugCmd>,
    },
    /// Choose how .test resolves: `hosts` (Private-Relay-safe) or `resolver`.
    Resolution {
        /// `hosts`, `resolver`, or omit to show the current mode.
        mode: Option<String>,
    },
    /// Take over ports 80/443 from Valet/Apache so dpl serves .test (sudo).
    Takeover,
    /// Reverse `takeover`: restore Valet and give back ports 80/443 (sudo).
    Untakeover,
    /// Undo `dpl setup`.
    Unsetup,
    /// Trust the local HTTPS CA in the system keychain (needs sudo).
    Trust,
    /// Remove the local CA from the system keychain (needs sudo).
    Untrust,

    /// List database/cache services and instances.
    Services,
    /// Manage services and multi-version instances.
    Service {
        /// list | versions | install | create | start | stop | restart | delete | info.
        action: String,
        /// Instance name; engine (versions); or engine[@version] (install).
        name: Option<String>,
        /// Engine for create/versions: postgres, mysql, mariadb, or redis.
        #[arg(long)]
        engine: Option<String>,
        /// Version for create (default: newest installed). See `service versions`.
        #[arg(long)]
        version: Option<String>,
        /// Port for create (default: first free above the engine default).
        #[arg(long)]
        port: Option<u16>,
    },
    /// Manage databases on a running service/instance.
    Db {
        /// list|create|drop|backup|restore <db>, or the branch-aware family:
        /// attach|detach|switch|branches|drop-branch <site> [branch].
        action: String,
        /// Database name (create/drop/backup/restore) or site name (branch family).
        name: Option<String>,
        /// Branch for `switch` (default: the checked-out branch) / `drop-branch`.
        branch: Option<String>,
        /// Engine: mysql or postgres.
        #[arg(long, default_value = "postgres")]
        engine: String,
        /// Target a specific instance port (default: the engine's default port).
        #[arg(long)]
        port: Option<u16>,
        /// Dump file for backup (out, default ~/.dpl/backups) / restore (in).
        #[arg(long)]
        file: Option<String>,
        /// Base database for `attach` (default: the project's .env DB_DATABASE).
        #[arg(long)]
        database: Option<String>,
    },
    /// Inspect captured local mail.
    Mail {
        #[command(subcommand)]
        command: Option<MailCmd>,
    },
    /// Manage the background daemon service (autostart on login).
    Daemon {
        /// One of: install, uninstall, start, stop, restart, status.
        action: String,
    },
    /// Import sites from an existing Laravel Valet install.
    Valet {
        #[command(subcommand)]
        cmd: ValetCmd,
    },
    /// Manage the TLDs your sites answer on (default: test).
    Tld {
        /// One of: list, add, remove, primary.
        #[arg(default_value = "list")]
        action: String,
        /// TLD name, e.g. `localhost` (for add/remove/primary).
        name: Option<String>,
    },

    /// Log in to dply via the device-authorization flow.
    Login(LoginArgs),
    /// Forget the stored token for the active host.
    Logout,
    /// Show the active host and cached profile.
    Whoami,

    /// dply platform commands (full parity with the dply CLI).
    #[command(subcommand)]
    Dply(DplyCommand),
}

#[derive(Args)]
pub struct LoginArgs {
    /// Don't try to open the verification URL in a browser.
    #[arg(long = "no-browser")]
    pub no_browser: bool,
}

/// Laravel Valet import operations.
#[derive(Subcommand)]
pub enum ValetCmd {
    /// List the parked directories and linked sites Valet has configured.
    List,
    /// Import Valet's parked dirs and linked sites into dpl.
    Import {
        /// Only import linked sites (skip parked directories).
        #[arg(long)]
        links_only: bool,
        /// Only import parked directories (skip linked sites).
        #[arg(long)]
        parks_only: bool,
        /// Also adopt Valet's TLD as dpl's primary domain.
        #[arg(long)]
        match_tld: bool,
        /// Import an explicit selection from a JSON manifest instead of all.
        #[arg(long)]
        manifest: Option<String>,
    },
    /// Remove imported sites (reverse of import). Without --manifest, removes
    /// everything Valet has.
    Remove {
        /// Remove an explicit selection from a JSON manifest.
        #[arg(long)]
        manifest: Option<String>,
    },
}

/// Captured-mail operations. `dpl mail` with no subcommand lists the inbox.
///
/// A message's mailbox is the SMTP username its sender authenticated with — set
/// `MAIL_USERNAME=<site>` in a project's `.env`. Use `-` to select mail that
/// arrived without one.
#[derive(Subcommand)]
pub enum MailCmd {
    /// List captured messages (the default with no subcommand).
    List {
        /// Only this mailbox (`-` for mail with no username).
        #[arg(long)]
        mailbox: Option<String>,
        /// Match against subject, sender, recipient and body text.
        #[arg(long)]
        search: Option<String>,
    },
    /// List mailboxes and how many messages each holds.
    Mailboxes,
    /// Print a message. Defaults to the raw source.
    Show {
        id: String,
        /// Which representation: raw | html | text | headers.
        #[arg(long, default_value = "raw")]
        part: String,
    },
    /// List a message's attachments.
    Attachments { id: String },
    /// Save one attachment to disk (index from `dpl mail attachments`).
    Save {
        id: String,
        index: usize,
        /// Destination file, or a directory to save into. Defaults to `.`.
        #[arg(long)]
        out: Option<String>,
    },
    /// Print every link in a message — password resets, verification URLs.
    Links { id: String },
    /// Send a test message into the sink, to check wiring or exercise the viewer.
    Send {
        #[arg(long, default_value = "dev@local.test")]
        to: String,
        #[arg(long, default_value = "dpl@local.test")]
        from: String,
        #[arg(long, default_value = "Test message from dpl")]
        subject: String,
        /// Authenticate as this mailbox (i.e. pretend to be that site).
        #[arg(long)]
        mailbox: Option<String>,
        /// Send a multipart HTML message rather than plain text.
        #[arg(long)]
        html: bool,
        /// Body text (a sample with a reset link is used when omitted).
        #[arg(long)]
        body: Option<String>,
    },
    /// Delete captured messages.
    Clear {
        /// Only this mailbox (`-` for mail with no username).
        #[arg(long)]
        mailbox: Option<String>,
    },
}

/// Per-project Node operations. dpl doesn't run Node — it manages each repo's
/// `.nvmrc` pin (which fnm/nvm auto-switch on) and installs versions through the
/// manager. `dpl node` with no subcommand lists each site's pinned version.
#[derive(Subcommand)]
pub enum NodeCmd {
    /// Show each site's pinned Node version and the detected manager (default).
    Status,
    /// Pin a Node version for a site by writing its `.nvmrc`.
    Use {
        /// Node version, e.g. `20` or `18.19.0`.
        version: String,
        /// Linked site name; omit to pin the current directory.
        site: Option<String>,
    },
    /// Install a Node version through the detected manager (fnm/nvm).
    Install {
        /// Node version, e.g. `20`.
        version: String,
    },
    /// Read a site's desired version from package.json and write its `.nvmrc`.
    Detect {
        /// Linked site name; omit for the current directory.
        site: Option<String>,
    },
}

/// SPX profiler operations. `dpl profile` with no subcommand lists each site's
/// status. Turning it on gives that site its own php-fpm master with SPX loaded,
/// auto-profiling every request into a same-origin flame-graph UI.
#[derive(Subcommand)]
pub enum ProfileCmd {
    /// Show each site's profiler status (the default with no subcommand).
    Status,
    /// Turn the profiler on for a site (installs SPX for its PHP if needed).
    On {
        /// Linked site name.
        site: String,
    },
    /// Turn the profiler off for a site.
    Off {
        /// Linked site name.
        site: String,
    },
    /// Open the flame-graph UI for a site in your browser.
    Open {
        /// Linked site name.
        site: String,
    },
}

/// Opcache preload operations. `dpl preload` with no subcommand shows each site's
/// status. A preloaded site runs on its own php-fpm master with `opcache.preload`
/// set, so its script's code is compiled into shared memory once at startup.
///
/// Preloaded entries are frozen for the master's life — edits don't take effect
/// until it restarts — so preload vendor/framework code, not the app code you're
/// actively editing. `generate` scaffolds a script with those defaults.
#[derive(Subcommand)]
pub enum PreloadCmd {
    /// Show each site's preload status (the default with no subcommand).
    Status,
    /// Scaffold a starter `dpl-preload.php` in the project (vendor-first).
    Generate {
        /// Linked site name; omit to use the current directory.
        site: Option<String>,
    },
    /// Turn preload on for a site (defaults to `dpl-preload.php`).
    On {
        /// Linked site name.
        site: String,
        /// Preload script path, relative to the project root.
        #[arg(long, default_value = "dpl-preload.php")]
        script: String,
    },
    /// Turn preload off for a site; it folds back into the shared master.
    Off {
        /// Linked site name.
        site: String,
    },
}

/// Xdebug operations. `dpl xdebug` with no subcommand shows the current mode
/// for every site.
///
/// A mode is a comma-separated list: `develop`, `coverage`, `debug`, `gcstats`,
/// `profile`, `trace` — or `off`. Sites with a mode of their own run on their
/// own php-fpm pool, so turning on step debugging for one site leaves the rest
/// untouched.
#[derive(Subcommand)]
pub enum XdebugCmd {
    /// Show each site's Xdebug mode (the default with no subcommand).
    Status,
    /// Turn on step debugging (mode `debug`).
    On {
        /// Linked site name; omit to set the default for all sites.
        site: Option<String>,
    },
    /// Turn Xdebug off.
    Off {
        /// Linked site name; omit to set the default for all sites.
        site: Option<String>,
    },
    /// Set an explicit mode, e.g. `debug,develop` or `profile`.
    Mode {
        /// Comma-separated modes, or `off`.
        mode: String,
        /// Linked site name; omit to set the default for all sites.
        site: Option<String>,
    },
    /// Set the port your IDE listens on (default 9003). Applies to all sites.
    Port {
        port: u16,
    },
    /// Set `xdebug.idekey` (e.g. PHPSTORM, VSCODE). Applies to all sites.
    Ide {
        key: String,
    },
}

/// PHP version-manager operations (Homebrew-backed). `dpl php` with no
/// subcommand still lists installed versions.
#[derive(Subcommand)]
pub enum PhpCmd {
    /// List all installable PHP versions with their status.
    Available,
    /// Install a PHP version via Homebrew, e.g. `dpl php install 8.3`.
    Install {
        /// PHP version, e.g. 8.3.
        version: String,
    },
    /// Upgrade a PHP version to the newest patch via Homebrew.
    Upgrade {
        /// PHP version, e.g. 8.3.
        version: String,
    },
    /// Uninstall a PHP version via Homebrew.
    Uninstall {
        /// PHP version, e.g. 8.0.
        version: String,
    },
    /// Repair a broken PHP install (reinstall keg and/or disable broken
    /// extensions), e.g. `dpl php repair 7.4`.
    Repair {
        /// PHP version, e.g. 7.4.
        version: String,
    },
    /// Disable extensions that fail to load (missing `.so`) without a reinstall.
    Fix {
        /// PHP version, e.g. 8.1.
        version: String,
    },
    /// List extensions installable for a version (shivammathur/extensions tap).
    ExtAvailable {
        /// PHP version, e.g. 8.3.
        version: String,
    },
    /// Install an extension for a version, e.g. `dpl php ext-install 8.3 swoole`.
    ExtInstall {
        /// PHP version, e.g. 8.3.
        version: String,
        /// Extension name, e.g. redis, swoole, mongodb.
        name: String,
    },
    /// Uninstall an extension for a version.
    ExtUninstall {
        /// PHP version, e.g. 8.3.
        version: String,
        /// Extension name.
        name: String,
    },
}

/// The dply subtree. Variant names use `group:command` to match the dply CLI.
#[derive(Subcommand)]
pub enum DplyCommand {
    // ---- auth (also available as top-level aliases) ----
    #[command(name = "login")]
    Login(LoginArgs),
    #[command(name = "logout")]
    Logout,
    #[command(name = "whoami")]
    Whoami,

    // ---- edge ----
    #[command(name = "edge:sites")]
    EdgeSites {
        #[arg(long)]
        status: Option<String>,
    },
    #[command(name = "edge:show")]
    EdgeShow { site: String },
    #[command(name = "edge:deploy")]
    EdgeDeploy {
        site: String,
        #[arg(long)]
        commit: Option<String>,
        #[arg(long)]
        branch: Option<String>,
    },
    #[command(name = "edge:deployments")]
    EdgeDeployments {
        site: String,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    #[command(name = "edge:deployment")]
    EdgeDeployment { site: String, deployment: String },
    #[command(name = "edge:rollback")]
    EdgeRollback {
        site: String,
        deployment: String,
        #[arg(long)]
        yes: bool,
    },
    #[command(name = "edge:access")]
    EdgeAccess {
        site: String,
        #[arg(long)]
        mode: Option<String>,
        #[arg(long)]
        password: Option<String>,
        #[arg(long = "allowed-email")]
        allowed_email: Vec<String>,
    },
    #[command(name = "edge:env")]
    EdgeEnv {
        site: String,
        #[arg(long = "set")]
        set: Vec<String>,
        #[arg(long = "unset")]
        unset: Vec<String>,
        #[arg(long = "from-file")]
        from_file: Option<String>,
        #[arg(long, default_value = "production")]
        scope: String,
    },
    #[command(name = "edge:domains")]
    EdgeDomains {
        site: String,
        #[arg(long)]
        add: Option<String>,
        #[arg(long)]
        verify: Option<String>,
        #[arg(long)]
        remove: Option<String>,
    },
    #[command(name = "edge:aliases")]
    EdgeAliases { site: String },
    #[command(name = "edge:previews")]
    EdgePreviews {
        site: String,
        #[arg(long)]
        create: Option<String>,
        #[arg(long)]
        delete: Option<String>,
        #[arg(long)]
        promote: Option<String>,
    },
    #[command(name = "edge:usage")]
    EdgeUsage {
        site: String,
        #[arg(long)]
        period: Option<String>,
    },
    #[command(name = "edge:purge")]
    EdgePurge { site: String, paths: Vec<String> },
    #[command(name = "edge:logs")]
    EdgeLogs {
        site: String,
        #[arg(long, default_value_t = 100)]
        limit: u32,
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        tail: bool,
        #[arg(long, default_value_t = 3)]
        interval: u64,
    },
    #[command(name = "edge:lint")]
    EdgeLint { path: Option<String> },

    // ---- servers ----
    #[command(name = "servers:list")]
    ServersList,
    #[command(name = "servers:run")]
    ServersRun {
        server: String,
        #[arg(long, default_value = "root")]
        user: String,
        #[arg(trailing_var_arg = true)]
        cmd: Vec<String>,
    },
    #[command(name = "servers:firewall")]
    ServersFirewall {
        server: String,
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        template: Option<String>,
        #[arg(long)]
        bundled: Option<String>,
    },
    #[command(name = "servers:log-shipping")]
    ServersLogShipping {
        server: String,
        #[arg(long)]
        enable: bool,
        #[arg(long)]
        resync: bool,
        #[arg(long)]
        disable: bool,
        #[arg(long = "source")]
        source: Vec<String>,
    },

    // ---- sites (server-hosted) ----
    #[command(name = "sites:list")]
    SitesList,
    #[command(name = "sites:show")]
    SitesShow { site: String },
    #[command(name = "sites:rename")]
    SitesRename {
        site: String,
        #[arg(long)]
        name: String,
    },
    #[command(name = "sites:deploy")]
    SitesDeploy { site: String },
    #[command(name = "sites:deployments")]
    SitesDeployments { site: String },
    #[command(name = "sites:deployment")]
    SitesDeployment { site: String, deployment: String },
    #[command(name = "sites:commits")]
    SitesCommits { site: String },
    #[command(name = "sites:domains:add")]
    SitesDomainsAdd {
        site: String,
        hostname: String,
        #[arg(long)]
        primary: bool,
        #[arg(long = "www-redirect")]
        www_redirect: bool,
    },
    #[command(name = "sites:domains:list")]
    SitesDomainsList { site: String },
    #[command(name = "sites:domains:remove")]
    SitesDomainsRemove { site: String, hostname: String },
    #[command(name = "sites:basic-auth:add")]
    SitesBasicAuthAdd {
        site: String,
        username: String,
        password: String,
        #[arg(long, default_value = "/")]
        path: String,
    },
    #[command(name = "sites:basic-auth:list")]
    SitesBasicAuthList { site: String },
    #[command(name = "sites:basic-auth:remove")]
    SitesBasicAuthRemove { site: String, username: String },
    #[command(name = "sites:db:list")]
    SitesDbList { site: String },
    #[command(name = "sites:schedules")]
    SitesSchedules { site: String },
    #[command(name = "sites:ssl:status")]
    SitesSslStatus { site: String },
    #[command(name = "sites:system-user")]
    SitesSystemUser { site: String },
    #[command(name = "sites:uptime")]
    SitesUptime { site: String },
    #[command(name = "sites:workers")]
    SitesWorkers { site: String },
    #[command(name = "sites:errors")]
    SitesErrors {
        site: String,
        #[arg(long, default_value = "20")]
        limit: String,
    },

    // ---- site (singular VM env) ----
    #[command(name = "site:env")]
    SiteEnv {
        site: String,
        #[arg(long = "set")]
        set: Vec<String>,
        #[arg(long = "unset")]
        unset: Vec<String>,
        #[arg(long = "from-file")]
        from_file: Option<String>,
    },

    // ---- insights / imports / operator ----
    #[command(name = "insights:summary")]
    InsightsSummary,
    #[command(name = "insights:server")]
    InsightsServer { server: String },
    #[command(name = "imports:migrations")]
    ImportsMigrations,
    #[command(name = "imports:migration")]
    ImportsMigration { migration: String },
    #[command(name = "operator:summary")]
    OperatorSummary,
    #[command(name = "operator:readme")]
    OperatorReadme {
        #[arg(long)]
        raw: bool,
    },
}
