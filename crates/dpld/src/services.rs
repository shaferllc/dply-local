//! Local database/cache services — DBngin-style.
//!
//! Two things coexist:
//!  1. **Managed instances** — named `(engine, version, port)` tuples the user
//!     creates (`dpl service create`), each with an isolated data dir under
//!     `~/.dpl/services/<name>`, supervised by the daemon. Multiple versions of
//!     an engine can run at once on different ports. Persisted in
//!     `~/.dpl/services.toml`.
//!  2. **External engines** — a DBngin / Postgres.app / brew instance already
//!     listening on an engine's default port. We detect and use it (databases,
//!     etc.) but don't supervise it.
//!
//! Engine binaries are discovered from Homebrew kegs and DBngin's bundled
//! versions, so `create` can pick any installed version.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use dpl_core::config::{ServiceInstance, ServicesConfig};
use dpl_core::ipc::{ServiceInfo, VersionInfo};
use tokio::process::{Child, Command};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Redis,
    Mysql,
    Postgres,
    Meilisearch,
    Mongodb,
    Minio,
    StripeMock,
}

impl Engine {
    fn name(&self) -> &'static str {
        match self {
            Engine::Redis => "redis",
            Engine::Mysql => "mysql",
            Engine::Postgres => "postgres",
            Engine::Meilisearch => "meilisearch",
            Engine::Mongodb => "mongodb",
            Engine::Minio => "minio",
            Engine::StripeMock => "stripe-mock",
        }
    }
    fn default_port(&self) -> u16 {
        match self {
            Engine::Redis => 6379,
            Engine::Mysql => 3306,
            Engine::Postgres => 5432,
            Engine::Meilisearch => 7700,
            Engine::Mongodb => 27017,
            Engine::Minio => 9000,
            Engine::StripeMock => 12111,
        }
    }
    fn server_name(&self) -> &'static [&'static str] {
        match self {
            Engine::Redis => &["redis-server"],
            Engine::Mysql => &["mariadbd", "mysqld"],
            Engine::Postgres => &["postgres"],
            Engine::Meilisearch => &["meilisearch"],
            Engine::Mongodb => &["mongod"],
            Engine::Minio => &["minio"],
            Engine::StripeMock => &["stripe-mock"],
        }
    }
    fn from_name(s: &str) -> Option<Engine> {
        match s.to_lowercase().as_str() {
            "redis" => Some(Engine::Redis),
            "mysql" | "mariadb" => Some(Engine::Mysql),
            "postgres" | "postgresql" | "pg" => Some(Engine::Postgres),
            "meilisearch" | "meili" => Some(Engine::Meilisearch),
            "mongodb" | "mongo" => Some(Engine::Mongodb),
            "minio" | "s3" | "rustfs" => Some(Engine::Minio),
            "stripe-mock" | "stripemock" | "stripe" => Some(Engine::StripeMock),
            _ => None,
        }
    }
    /// SQL engines that support the `db` sub-commands.
    fn has_databases(&self) -> bool {
        matches!(self, Engine::Mysql | Engine::Postgres)
    }
    const ALL: [Engine; 7] = [
        Engine::Redis, Engine::Mysql, Engine::Postgres,
        Engine::Meilisearch, Engine::Mongodb, Engine::Minio, Engine::StripeMock,
    ];
}

pub struct Services {
    config: ServicesConfig,
    config_path: PathBuf,
    /// Running managed instances, keyed by instance name.
    running: BTreeMap<String, Child>,
}

impl Services {
    pub fn new() -> Self {
        let config_path = dpl_core::paths::services_config(None).unwrap_or_default();
        let config = ServicesConfig::load(&config_path).unwrap_or_default();
        Services { config, config_path, running: BTreeMap::new() }
    }

    fn save(&self) -> Result<()> {
        self.config.save(&self.config_path).context("saving services.toml")
    }

    /// Installed engine versions we can create instances from.
    pub fn available_versions(&self, engine: Option<&str>) -> Vec<VersionInfo> {
        let engines: Vec<Engine> = match engine.and_then(Engine::from_name) {
            Some(e) => vec![e],
            None => Engine::ALL.to_vec(),
        };
        let mut out = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for e in engines {
            for dir in engine_bin_dirs(e) {
                if e.server_name().iter().any(|n| dir.join(n).is_file()) {
                    let version = version_of_bindir(&dir);
                    // Skip unversioned Homebrew symlink dirs (`postgresql`, `mariadb`).
                    if !version.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                        continue;
                    }
                    if seen.insert((e.name(), version.clone())) {
                        out.push(VersionInfo {
                            engine: e.name().to_string(),
                            version,
                            source: source_of(&dir).to_string(),
                        });
                    }
                }
            }
        }
        out
    }

    /// Managed instances plus any external default-port engines.
    pub fn list(&self) -> Vec<ServiceInfo> {
        let mut out: Vec<ServiceInfo> = Vec::new();
        let mut ports_taken = std::collections::BTreeSet::new();

        for inst in &self.config.instances {
            ports_taken.insert(inst.port);
            out.push(ServiceInfo {
                name: inst.name.clone(),
                engine: inst.engine.clone(),
                port: inst.port,
                installed: true,
                running: self.running.contains_key(&inst.name),
                external: false,
                version: inst.version.clone(),
            });
        }

        // External engines on their default port (DBngin/Postgres.app/brew).
        for e in Engine::ALL {
            let port = e.default_port();
            if !ports_taken.contains(&port) && port_open(port) {
                out.push(ServiceInfo {
                    name: e.name().to_string(),
                    engine: e.name().to_string(),
                    port,
                    installed: true,
                    running: true,
                    external: true,
                    version: String::new(),
                });
            }
        }
        out
    }

    /// Start managed instances that should be running; stop removed ones.
    pub fn reconcile(&mut self) -> usize {
        let names: std::collections::BTreeSet<String> =
            self.config.instances.iter().map(|i| i.name.clone()).collect();
        let stale: Vec<String> = self.running.keys().filter(|n| !names.contains(*n)).cloned().collect();
        for n in stale {
            if let Some(mut c) = self.running.remove(&n) {
                let _ = c.start_kill();
            }
        }
        let instances = self.config.instances.clone();
        for inst in instances {
            if !self.running.contains_key(&inst.name) && !port_open(inst.port) {
                if let Err(e) = self.spawn_instance(&inst) {
                    tracing::warn!(instance = %inst.name, error = %e, "could not start instance");
                }
            }
        }
        self.running.len()
    }

    // ---- instance lifecycle ----

    pub fn create(&mut self, name: &str, engine: &str, version: Option<&str>, port: Option<u16>) -> Result<String> {
        valid_name(name)?;
        let e = Engine::from_name(engine).with_context(|| format!("unknown engine: {engine}"))?;
        if self.config.instances.iter().any(|i| i.name == name) {
            bail!("an instance named `{name}` already exists");
        }
        // Pick a version: the requested one, else the newest available.
        let versions = self.available_versions(Some(e.name()));
        if versions.is_empty() {
            bail!("no {} versions found (install one via DBngin or Homebrew)", e.name());
        }
        let version = match version {
            Some(v) => versions.iter().find(|x| x.version == v)
                .with_context(|| format!("{} {v} not installed. Available: {}", e.name(),
                    versions.iter().map(|x| x.version.as_str()).collect::<Vec<_>>().join(", ")))?
                .version.clone(),
            None => versions[0].version.clone(),
        };
        // Pick a port: requested (must be free) or first free from default+1.
        let port = match port {
            Some(p) => {
                if port_open(p) || self.config.instances.iter().any(|i| i.port == p) {
                    bail!("port {p} is already in use");
                }
                p
            }
            None => self.free_port(e),
        };

        let inst = ServiceInstance { name: name.to_string(), engine: e.name().to_string(), version, port };
        self.spawn_instance(&inst)?;
        self.config.instances.push(inst.clone());
        self.save()?;
        Ok(format!("Created {} instance `{name}` ({} {}) on port {port}.", e.name(), e.name(), inst.version))
    }

    pub fn start(&mut self, name: &str) -> Result<String> {
        // Instance name?
        if let Some(inst) = self.config.instances.iter().find(|i| i.name == name).cloned() {
            if self.running.contains_key(name) {
                return Ok(format!("`{name}` is already running."));
            }
            self.spawn_instance(&inst)?;
            return Ok(format!("Started `{name}` on port {}.", inst.port));
        }
        // Else an engine name → external default port.
        let e = Engine::from_name(name).with_context(|| format!("no instance or engine named `{name}`"))?;
        if port_open(e.default_port()) {
            return Ok(format!("{} is already running on port {} (managed externally).", e.name(), e.default_port()));
        }
        bail!("no managed instance `{name}` — create one with `dpl service create {name} --engine {name}`")
    }

    pub fn stop(&mut self, name: &str) -> Result<String> {
        match self.running.remove(name) {
            Some(mut c) => { let _ = c.start_kill(); Ok(format!("Stopped `{name}`.")) }
            None => {
                if let Some(e) = Engine::from_name(name) {
                    if port_open(e.default_port()) {
                        return Ok(format!("{} is managed externally (e.g. DBngin) — stop it there.", e.name()));
                    }
                }
                Ok(format!("`{name}` was not running."))
            }
        }
    }

    pub fn restart(&mut self, name: &str) -> Result<String> {
        let _ = self.stop(name);
        std::thread::sleep(std::time::Duration::from_millis(400));
        self.start(name)
    }

    pub fn delete(&mut self, name: &str) -> Result<String> {
        if let Some(mut c) = self.running.remove(name) {
            let _ = c.start_kill();
        }
        let before = self.config.instances.len();
        self.config.instances.retain(|i| i.name != name);
        if self.config.instances.len() == before {
            bail!("no managed instance `{name}`");
        }
        self.save()?;
        Ok(format!("Deleted instance `{name}` (data dir left under ~/.dpl/services/{name})."))
    }

    fn free_port(&self, engine: Engine) -> u16 {
        let mut p = engine.default_port() + 1;
        loop {
            let taken = port_open(p) || self.config.instances.iter().any(|i| i.port == p);
            if !taken {
                return p;
            }
            p += 1;
        }
    }

    fn spawn_instance(&mut self, inst: &ServiceInstance) -> Result<()> {
        let e = Engine::from_name(&inst.engine).context("bad engine")?;
        let bin_dir = bin_dir_for(e, &inst.version)
            .with_context(|| format!("{} {} binaries not found", inst.engine, inst.version))?;
        let data = dpl_core::paths::services_dir(None)?.join(&inst.name).join("data");
        std::fs::create_dir_all(&data).ok();
        init_instance(e, &bin_dir, &data)?;
        let child = spawn_server(e, &bin_dir, &data, inst.port, &inst.name)?;
        tracing::info!(instance = %inst.name, engine = %inst.engine, version = %inst.version, port = inst.port, "instance started");
        self.running.insert(inst.name.clone(), child);
        Ok(())
    }

    // ---- databases ----

    pub fn db(&self, action: &str, engine: &str, name: Option<&str>, port: Option<u16>, file: Option<&str>) -> Result<String> {
        let e = Engine::from_name(engine)
            .with_context(|| format!("db engine must be mysql or postgres, got {engine}"))?;
        if !e.has_databases() {
            bail!("`db` commands apply to mysql/postgres only, not {engine}");
        }
        let port = port.unwrap_or_else(|| e.default_port());
        if !port_open(port) {
            bail!("nothing is listening on {engine} port {port} — start it (or DBngin) first.");
        }
        match action {
            "list" => db_list(e, port),
            "create" => db_create(e, port, name.context("database name required")?),
            "drop" => db_drop(e, port, name.context("database name required")?),
            "backup" => db_backup(e, port, name.context("database name required")?, file),
            "restore" => db_restore(e, port, name.context("database name required")?, file.context("--file required")?),
            other => bail!("unknown db action: {other}"),
        }
    }
}

// ---- binary discovery ----

pub(crate) fn port_open(port: u16) -> bool {
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(250)).is_ok()
}

/// Candidate `bin` dirs for an engine, newest-version first: Homebrew kegs then
/// DBngin bundled versions.
fn engine_bin_dirs(engine: Engine) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    let keg_matches = |dir: &str| match engine {
        Engine::Redis => dir.starts_with("redis") && dir != "redis-cli",
        Engine::Mysql => dir.starts_with("mariadb") || (dir.starts_with("mysql") && !dir.contains("client")),
        Engine::Postgres => dir.starts_with("postgresql"),
        Engine::Meilisearch => dir == "meilisearch",
        Engine::Mongodb => dir.starts_with("mongodb") && !dir.contains("database-tools"),
        Engine::Minio => dir == "minio",
        Engine::StripeMock => dir == "stripe-mock",
    };
    for prefix in ["/opt/homebrew/opt", "/usr/local/opt"] {
        if let Ok(entries) = std::fs::read_dir(prefix) {
            let mut kegs: Vec<String> = entries.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|d| keg_matches(d)).collect();
            kegs.sort();
            kegs.reverse();
            for keg in kegs {
                dirs.push(PathBuf::from(prefix).join(keg).join("bin"));
            }
        }
    }

    let dbngin_subs: &[&str] = match engine {
        Engine::Redis => &["redis"],
        Engine::Mysql => &["mysql", "mariadb"],
        Engine::Postgres => &["postgresql"],
        // Not bundled by DBngin.
        Engine::Meilisearch | Engine::Mongodb | Engine::Minio | Engine::StripeMock => &[],
    };
    for sub in dbngin_subs {
        let base = PathBuf::from("/Users/Shared/DBngin").join(sub);
        if let Ok(entries) = std::fs::read_dir(&base) {
            let mut vers: Vec<String> = entries.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            vers.sort_by(|a, b| version_key(b).cmp(&version_key(a)));
            for v in vers {
                dirs.push(base.join(v).join("bin"));
            }
        }
    }
    dirs
}

fn bin_dir_for(engine: Engine, version: &str) -> Option<PathBuf> {
    engine_bin_dirs(engine).into_iter().find(|d| {
        version_of_bindir(d) == version && engine.server_name().iter().any(|n| d.join(n).is_file())
    })
}

/// The version label for a bin dir: the parent folder name, keeping the part
/// after `@` for Homebrew kegs (`postgresql@17` → `17`; DBngin `17.0` → `17.0`).
fn version_of_bindir(bin_dir: &Path) -> String {
    let ver = bin_dir.parent().and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    ver.rsplit('@').next().unwrap_or(&ver).to_string()
}

fn source_of(bin_dir: &Path) -> &'static str {
    if bin_dir.to_string_lossy().contains("/DBngin/") { "dbngin" } else { "homebrew" }
}

fn version_key(v: &str) -> Vec<u32> {
    v.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect()
}

// ---- init + spawn ----

fn init_instance(engine: Engine, bin_dir: &Path, data: &Path) -> Result<()> {
    let populated = std::fs::read_dir(data).map(|mut d| d.next().is_some()).unwrap_or(false);
    if populated {
        return Ok(());
    }
    match engine {
        // These just need their (already-created) data dir.
        Engine::Redis | Engine::Meilisearch | Engine::Mongodb | Engine::Minio | Engine::StripeMock => Ok(()),
        Engine::Postgres => {
            let initdb = bin_dir.join("initdb");
            run(&initdb, &["-D", &data.to_string_lossy(), "-U", "postgres", "--auth=trust", "-E", "UTF8"])
                .context("initialising PostgreSQL data dir")
        }
        Engine::Mysql => {
            let installer = ["mariadb-install-db", "mysql_install_db"].iter()
                .map(|t| bin_dir.join(t)).find(|p| p.is_file());
            if let Some(installer) = installer {
                run(&installer, &[&format!("--datadir={}", data.display()), "--auth-root-authentication-method=normal"])
                    .or_else(|_| run(&installer, &[&format!("--datadir={}", data.display())]))
                    .context("initialising MariaDB/MySQL data dir")
            } else {
                let server = bin_dir.join(server_bin_name(engine, bin_dir));
                run(&server, &["--initialize-insecure", &format!("--datadir={}", data.display())])
                    .context("initialising MySQL data dir")
            }
        }
    }
}

fn spawn_server(engine: Engine, bin_dir: &Path, data: &Path, port: u16, name: &str) -> Result<Child> {
    let server = bin_dir.join(server_bin_name(engine, bin_dir));
    let log = log_file(name)?;
    let errlog = log.try_clone().ok();
    let mut cmd = Command::new(&server);
    match engine {
        Engine::Redis => {
            cmd.args(["--port", &port.to_string(), "--daemonize", "no", "--save", "", "--dir"]).arg(data);
        }
        Engine::Postgres => {
            cmd.args(["-D", &data.to_string_lossy(), "-p", &port.to_string()]);
        }
        Engine::Mysql => {
            cmd.arg(format!("--datadir={}", data.display()))
                .arg(format!("--port={port}"))
                .arg(format!("--socket={}/mysql.sock", data.display()))
                .arg(format!("--pid-file={}/mysqld.pid", data.display()));
        }
        Engine::Meilisearch => {
            cmd.args(["--http-addr", &format!("127.0.0.1:{port}"), "--no-analytics", "--env", "development", "--db-path"]).arg(data);
        }
        Engine::Mongodb => {
            cmd.args(["--dbpath", &data.to_string_lossy(), "--port", &port.to_string(), "--bind_ip", "127.0.0.1"]);
        }
        Engine::Minio => {
            // S3-compatible object store; console picks an ephemeral port.
            cmd.env("MINIO_ROOT_USER", "minioadmin")
                .env("MINIO_ROOT_PASSWORD", "minioadmin")
                .args(["server", &data.to_string_lossy(), "--address", &format!("127.0.0.1:{port}")]);
        }
        Engine::StripeMock => {
            cmd.args(["-http-port", &port.to_string()]);
        }
    }
    cmd.stdout(std::process::Stdio::from(log))
        .stderr(errlog.map(std::process::Stdio::from).unwrap_or_else(std::process::Stdio::null))
        .kill_on_drop(true);
    cmd.spawn().with_context(|| format!("spawning {}", server.display()))
}

/// Which server binary name exists in this bin dir.
fn server_bin_name(engine: Engine, bin_dir: &Path) -> &'static str {
    engine.server_name().iter().copied().find(|n| bin_dir.join(n).is_file())
        .unwrap_or(engine.server_name()[0])
}

fn log_file(name: &str) -> Result<std::fs::File> {
    let dir = dpl_core::paths::logs_dir(None)?;
    std::fs::create_dir_all(&dir).ok();
    std::fs::File::create(dir.join(format!("service-{name}.log"))).context("service log")
}

// ---- db client operations ----

fn db_list(engine: Engine, port: u16) -> Result<String> {
    match engine {
        Engine::Postgres => {
            let psql = client(engine, "psql")?;
            output(&psql, &["-h", "127.0.0.1", "-p", &port.to_string(), "-U", "postgres",
                "-Atc", "SELECT datname FROM pg_database WHERE datistemplate=false ORDER BY datname"])
        }
        Engine::Mysql => {
            let mysql = client(engine, "mysql")?;
            output(&mysql, &["-h", "127.0.0.1", "-P", &port.to_string(), "-uroot", "-N", "-e", "SHOW DATABASES"])
        }
        _ => bail!("this service has no SQL databases"),
    }
}

fn db_create(engine: Engine, port: u16, name: &str) -> Result<String> {
    valid_db_name(name)?;
    match engine {
        Engine::Postgres => {
            let createdb = client(engine, "createdb")?;
            run(&createdb, &["-h", "127.0.0.1", "-p", &port.to_string(), "-U", "postgres", name])?;
        }
        Engine::Mysql => {
            let mysql = client(engine, "mysql")?;
            run(&mysql, &["-h", "127.0.0.1", "-P", &port.to_string(), "-uroot", "-e", &format!("CREATE DATABASE `{name}`")])?;
        }
        _ => bail!("this service has no SQL databases"),
    }
    Ok(format!("Created database `{name}`."))
}

fn db_drop(engine: Engine, port: u16, name: &str) -> Result<String> {
    valid_db_name(name)?;
    match engine {
        Engine::Postgres => {
            let dropdb = client(engine, "dropdb")?;
            run(&dropdb, &["-h", "127.0.0.1", "-p", &port.to_string(), "-U", "postgres", "--if-exists", name])?;
        }
        Engine::Mysql => {
            let mysql = client(engine, "mysql")?;
            run(&mysql, &["-h", "127.0.0.1", "-P", &port.to_string(), "-uroot", "-e", &format!("DROP DATABASE IF EXISTS `{name}`")])?;
        }
        _ => bail!("this service has no SQL databases"),
    }
    Ok(format!("Dropped database `{name}`."))
}

/// Back up a database to a `.sql` file (plain SQL dump). Defaults the path to
/// `~/.dpl/backups/<engine>-<db>.sql` when none is given.
fn db_backup(engine: Engine, port: u16, db: &str, file: Option<&str>) -> Result<String> {
    valid_db_name(db)?;
    let out_path = match file {
        Some(f) => PathBuf::from(f),
        None => {
            let dir = dpl_core::paths::dpl_dir(None)?.join("backups");
            std::fs::create_dir_all(&dir).ok();
            dir.join(format!("{}-{db}.sql", engine.name()))
        }
    };
    let (tool, args): (&str, Vec<String>) = match engine {
        Engine::Postgres => ("pg_dump", vec![
            "-h".into(), "127.0.0.1".into(), "-p".into(), port.to_string(),
            "-U".into(), "postgres".into(), db.into()]),
        Engine::Mysql => ("mysqldump", vec![
            "-h".into(), "127.0.0.1".into(), "-P".into(), port.to_string(),
            "-uroot".into(), db.into()]),
        _ => bail!("backup not supported for this service"),
    };
    let bin = client(engine, tool)?;
    let dump = std::process::Command::new(&bin)
        .args(&args)
        .output()
        .with_context(|| format!("running {tool}"))?;
    if !dump.status.success() {
        bail!("{tool} failed: {}", String::from_utf8_lossy(&dump.stderr).trim());
    }
    std::fs::write(&out_path, &dump.stdout).with_context(|| format!("writing {}", out_path.display()))?;
    Ok(format!("Backed up `{db}` to {} ({} KB).", out_path.display(), dump.stdout.len() / 1024))
}

/// Restore a database from a `.sql` dump (creates the DB if needed).
fn db_restore(engine: Engine, port: u16, db: &str, file: &str) -> Result<String> {
    valid_db_name(db)?;
    let sql = std::fs::read(file).with_context(|| format!("reading {file}"))?;
    // Ensure the target database exists.
    let _ = db_create(engine, port, db);

    let (tool, args): (&str, Vec<String>) = match engine {
        Engine::Postgres => ("psql", vec![
            "-h".into(), "127.0.0.1".into(), "-p".into(), port.to_string(),
            "-U".into(), "postgres".into(), "-d".into(), db.into()]),
        Engine::Mysql => ("mysql", vec![
            "-h".into(), "127.0.0.1".into(), "-P".into(), port.to_string(),
            "-uroot".into(), db.into()]),
        _ => bail!("restore not supported for this service"),
    };
    let bin = client(engine, tool)?;
    let mut child = std::process::Command::new(&bin)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("running {tool}"))?;
    use std::io::Write;
    child.stdin.take().context("no stdin")?.write_all(&sql).context("piping dump")?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("{tool} restore failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(format!("Restored `{db}` from {file}."))
}

// ---- branch-aware databases (Postgres) ----
//
// The scheme (see dpl_core::branchdb): the base database always holds the
// checked-out branch's data; other branches are parked as `<base>@<branch>`.
// A switch is two catalog renames (~100ms); only a branch's first visit pays
// a `CREATE DATABASE ... TEMPLATE` copy (~2.5s per 250MB, measured).

/// Run one SQL statement against the maintenance DB, returning stdout.
fn pg_admin(port: u16, sql: &str) -> Result<String> {
    let psql = client(Engine::Postgres, "psql")?;
    let out = std::process::Command::new(&psql)
        .args(["-h", "127.0.0.1", "-p", &port.to_string(), "-U", "postgres",
               "-v", "ON_ERROR_STOP=1", "-d", "postgres", "-Atc", sql])
        .output()
        .with_context(|| format!("running {}", psql.display()))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        bail!("psql failed: {}", String::from_utf8_lossy(&out.stderr).trim())
    }
}

fn pg_db_exists(port: u16, name: &str) -> Result<bool> {
    Ok(pg_admin(port, &format!("SELECT 1 FROM pg_database WHERE datname = '{name}'"))? == "1")
}

/// Kick every session off a database so it can be renamed or templated.
/// In a dev environment this is safe: php-fpm reconnects on the next request.
fn pg_terminate(port: u16, name: &str) -> Result<()> {
    pg_admin(port, &format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
         WHERE datname = '{name}' AND pid <> pg_backend_pid()"
    ))?;
    Ok(())
}

/// Guard every name headed into SQL. All callers pass through here.
fn pg_ident(name: &str) -> Result<&str> {
    if dpl_core::branchdb::safe_identifier(name) {
        Ok(name)
    } else {
        bail!("unusable database identifier: {name}")
    }
}

/// Ensure the base database exists (for attach). Returns true if created.
pub fn pg_ensure_db(port: u16, base: &str) -> Result<bool> {
    let base = pg_ident(base)?;
    if pg_db_exists(port, base)? {
        return Ok(false);
    }
    pg_admin(port, &format!("CREATE DATABASE \"{base}\""))?;
    Ok(true)
}

/// Move the live database from branch `from` to branch `to`.
///
/// The base is renamed to park `from`'s data; `to`'s parked data is renamed
/// into place when it exists, else cloned from `from` (a new branch starts
/// from the state you left — matching how the code diverged).
pub fn pg_branch_switch(port: u16, base: &str, from: &str, to: &str) -> Result<String> {
    let base = pg_ident(base)?;
    let park_from = dpl_core::branchdb::branch_db_name(base, from);
    let park_to = dpl_core::branchdb::branch_db_name(base, to);
    let (park_from, park_to) = (pg_ident(&park_from)?, pg_ident(&park_to)?);

    if !pg_db_exists(port, base)? {
        bail!("database `{base}` does not exist — create it (or run migrations) first");
    }

    // Park the live data under its branch. A parked copy of `from` can already
    // exist after an interrupted switch; the live base is the fresher truth.
    pg_terminate(port, base)?;
    if pg_db_exists(port, park_from)? {
        pg_admin(port, &format!("DROP DATABASE \"{park_from}\""))?;
    }
    pg_admin(port, &format!("ALTER DATABASE \"{base}\" RENAME TO \"{park_from}\""))?;

    // Bring `to`'s data live: instant rename when the branch was visited
    // before, one-time template copy when it's new.
    let created = if pg_db_exists(port, park_to)? {
        pg_terminate(port, park_to)?;
        pg_admin(port, &format!("ALTER DATABASE \"{park_to}\" RENAME TO \"{base}\""))?;
        false
    } else {
        pg_admin(port, &format!("CREATE DATABASE \"{base}\" TEMPLATE \"{park_from}\""))?;
        true
    };

    Ok(if created {
        format!("`{base}` now tracks `{to}` (new branch — cloned from `{from}`).")
    } else {
        format!("`{base}` now tracks `{to}` (swapped, `{from}` parked).")
    })
}

/// Parked branch databases for a base, as `branch<TAB>size` lines.
pub fn pg_branch_list(port: u16, base: &str) -> Result<Vec<String>> {
    let base = pg_ident(base)?;
    // Escape LIKE wildcards in the base name (`my_app` contains `_`).
    let pattern = base.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    let out = pg_admin(port, &format!(
        "SELECT datname || E'\\t' || pg_size_pretty(pg_database_size(datname)) \
         FROM pg_database WHERE datname LIKE '{pattern}@%' ESCAPE '\\' ORDER BY datname"
    ))?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let (db, size) = l.split_once('\t')?;
            let branch = dpl_core::branchdb::branch_of_db(base, db)?;
            Some(format!("{branch}\t{size}"))
        })
        .collect())
}

/// Drop one parked branch database. Refuses the live base by construction
/// (only `<base>@<branch>` names are ever dropped).
pub fn pg_branch_drop(port: u16, base: &str, branch: &str) -> Result<String> {
    let base = pg_ident(base)?;
    let park = dpl_core::branchdb::branch_db_name(base, branch);
    let park = pg_ident(&park)?;
    if !pg_db_exists(port, park)? {
        return Ok(format!("no parked database for branch `{branch}`."));
    }
    pg_terminate(port, park)?;
    pg_admin(port, &format!("DROP DATABASE \"{park}\""))?;
    Ok(format!("Dropped parked database for `{branch}`."))
}

/// The size of one database, human-formatted (for `branches` output).
pub fn pg_db_size(port: u16, name: &str) -> Result<String> {
    let name = pg_ident(name)?;
    pg_admin(port, &format!("SELECT pg_size_pretty(pg_database_size('{name}'))"))
}

fn client(engine: Engine, tool: &str) -> Result<PathBuf> {
    for dir in engine_bin_dirs(engine) {
        let cand = dir.join(tool);
        if cand.is_file() {
            return Ok(cand);
        }
    }
    which(tool).with_context(|| format!("{tool} not found (install it or DBngin's client tools)"))
}

fn valid_name(name: &str) -> Result<()> {
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        Ok(())
    } else {
        bail!("instance name must be alphanumeric/-/_: {name}")
    }
}

fn valid_db_name(name: &str) -> Result<()> {
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Ok(())
    } else {
        bail!("database name must be alphanumeric/underscore: {name}")
    }
}

fn run(program: &Path, args: &[&str]) -> Result<()> {
    let out = std::process::Command::new(program).args(args).output()
        .with_context(|| format!("running {}", program.display()))?;
    if out.status.success() { Ok(()) }
    else { bail!("{} failed: {}", program.display(), String::from_utf8_lossy(&out.stderr).trim()) }
}

fn output(program: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new(program).args(args).output()
        .with_context(|| format!("running {}", program.display()))?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(if s.is_empty() { "(none)".into() } else { s })
    } else {
        bail!("{} failed: {}", program.display(), String::from_utf8_lossy(&out.stderr).trim())
    }
}

use dpl_core::tools::which;
