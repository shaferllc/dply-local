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
    /// List installed PHP versions.
    Php,
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
    /// Share a local site publicly via a Cloudflare quick tunnel.
    Share {
        /// Site name (default: current directory's name).
        name: Option<String>,
    },
    /// Reload the config from disk and restart backends.
    Restart,
    /// Show config, socket, and data paths.
    Paths,
    /// Check the local environment and report problems.
    Doctor,
    /// One-time privileged setup: trust the CA, route .test, redirect :80/:443.
    Setup {
        /// Skip the :80/:443 port redirect (sites stay on :8080/:8443).
        #[arg(long)]
        no_ports: bool,
    },
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
        /// One of: list, create, drop, backup, restore.
        action: String,
        /// Database name (for create/drop/backup/restore).
        name: Option<String>,
        /// Engine: mysql or postgres.
        #[arg(long, default_value = "postgres")]
        engine: String,
        /// Target a specific instance port (default: the engine's default port).
        #[arg(long)]
        port: Option<u16>,
        /// Dump file for backup (out, default ~/.dpl/backups) / restore (in).
        #[arg(long)]
        file: Option<String>,
    },
    /// Inspect captured local mail.
    Mail {
        /// One of: list, show, clear.
        #[arg(default_value = "list")]
        action: String,
        /// Message id (for show).
        id: Option<String>,
    },
    /// Manage the background daemon service (autostart on login).
    Daemon {
        /// One of: install, uninstall, start, stop, restart, status.
        action: String,
    },
    /// Manage the TLDs your sites answer on (default: test).
    Tld {
        /// One of: list, add, remove.
        #[arg(default_value = "list")]
        action: String,
        /// TLD name, e.g. `localhost` (for add/remove).
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
