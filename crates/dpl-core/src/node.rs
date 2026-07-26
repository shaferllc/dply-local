//! Per-project Node version management, delegated to fnm/nvm.
//!
//! dpl doesn't run `node` (you do, in your shell), so it can't force a version
//! the way it does for php-fpm. Instead it manages the pieces a Node version
//! manager needs: it writes each repo's `.nvmrc` — the pin file both fnm and nvm
//! read and auto-switch on — and reads a project's desired version back out of
//! `.nvmrc`, `.node-version`, or `package.json` `engines.node`. The manager does
//! the actual switching when you `cd` in.
//!
//! One exception: when dpl runs a command *for* you across sites (`dpl node
//! deps`, `dpl node run`), there is no `cd` to trigger the switch, so it asks
//! the manager to apply the pin for that one invocation — see
//! [`pinned_invocation`]. Those commands also work out *which* package manager
//! each project uses — npm, pnpm, Yarn, or Bun — see [`detect_agent`], because a
//! fleet is rarely all one thing and `npm install` in a pnpm repo is a mess to
//! undo.

use std::path::Path;

/// A Node version manager dpl can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    /// fnm — a fast, single-binary manager (`brew install fnm`).
    Fnm,
    /// nvm — the shell-function manager, loaded from `~/.nvm/nvm.sh`.
    Nvm,
}

impl Manager {
    pub fn name(self) -> &'static str {
        match self {
            Manager::Fnm => "fnm",
            Manager::Nvm => "nvm",
        }
    }
}

/// The Node manager available on this machine, preferring fnm (a real binary we
/// can call directly) over nvm (a shell function we have to source).
pub fn detect_manager() -> Option<Manager> {
    if crate::tools::which("fnm").is_some() {
        return Some(Manager::Fnm);
    }
    if nvm_script().is_some() {
        return Some(Manager::Nvm);
    }
    None
}

/// Path to `nvm.sh`, honouring `$NVM_DIR`, else `~/.nvm`.
pub fn nvm_script() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("NVM_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".nvm")))?;
    let script = dir.join("nvm.sh");
    script.is_file().then_some(script)
}

/// Where a project's pinned Node version comes from, and what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// The version string, as written for the manager (e.g. `"20"`, `"18.19.0"`,
    /// `"lts/*"`).
    pub version: String,
    /// Which file it was read from, for display.
    pub source: PinSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinSource {
    Nvmrc,
    NodeVersion,
    PackageJson,
}

impl PinSource {
    pub fn as_str(self) -> &'static str {
        match self {
            PinSource::Nvmrc => ".nvmrc",
            PinSource::NodeVersion => ".node-version",
            PinSource::PackageJson => "package.json",
        }
    }
}

/// Read a project's pinned Node version: `.nvmrc` and `.node-version` win (they're
/// the manager's own pins), else `package.json` `engines.node` as a fallback
/// hint. `None` when the project says nothing about Node.
pub fn read_pin(project_dir: &Path) -> Option<Pin> {
    if let Some(v) = read_first_line(&project_dir.join(".nvmrc")) {
        return Some(Pin { version: v, source: PinSource::Nvmrc });
    }
    if let Some(v) = read_first_line(&project_dir.join(".node-version")) {
        return Some(Pin { version: v, source: PinSource::NodeVersion });
    }
    engines_node(project_dir).map(|v| Pin { version: v, source: PinSource::PackageJson })
}

/// Write `.nvmrc` in `project_dir`, the pin fnm and nvm both auto-switch on.
pub fn write_nvmrc(project_dir: &Path, version: &str) -> Result<(), crate::error::CoreError> {
    let path = project_dir.join(".nvmrc");
    let body = format!("{}\n", version.trim());
    std::fs::write(&path, body).map_err(|e| crate::error::CoreError::io(&path, e))
}

/// Reduce a `package.json` `engines.node` range to a version a manager accepts —
/// the first version number in the range (`"^20.11 || 18"` → `"20"`, `">=18"` →
/// `"18"`). A hint, not a resolution; the user confirms with `dpl node use`.
pub fn normalize_range(range: &str) -> Option<String> {
    let mut digits = String::new();
    for ch in range.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else if !digits.is_empty() {
            break;
        }
    }
    (!digits.is_empty()).then_some(digits)
}

/// A JavaScript package manager dpl can drive on a project's behalf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl Agent {
    /// The binary to spawn — also the name we print.
    pub fn as_str(self) -> &'static str {
        match self {
            Agent::Npm => "npm",
            Agent::Pnpm => "pnpm",
            Agent::Yarn => "yarn",
            Agent::Bun => "bun",
        }
    }

    /// Parse a user-supplied agent name (`--agent pnpm`).
    pub fn parse(name: &str) -> Option<Agent> {
        match name.trim().to_lowercase().as_str() {
            "npm" => Some(Agent::Npm),
            "pnpm" => Some(Agent::Pnpm),
            "yarn" => Some(Agent::Yarn),
            "bun" => Some(Agent::Bun),
            _ => None,
        }
    }

    /// Every agent name, for error messages.
    pub const ALL: [Agent; 4] = [Agent::Npm, Agent::Pnpm, Agent::Yarn, Agent::Bun];
}

/// Which agent a project uses, what said so, and the one dialect wrinkle that
/// can't be answered by the name alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChoice {
    pub agent: Agent,
    pub reason: AgentReason,
    /// Yarn 2+ ("berry"), whose flags diverge from Yarn 1's — `--immutable`
    /// where classic Yarn wants `--frozen-lockfile`. Meaningless for the others.
    pub berry: bool,
}

/// What identified the agent, kept for display: a guess the user can't see is a
/// guess they can't correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentReason {
    /// package.json's `packageManager` field — the Corepack declaration.
    Declared,
    /// A lockfile in the project root.
    Lockfile(&'static str),
    /// Nothing said; npm is the assumption.
    Assumed,
    /// The user overrode detection for this run (`--agent`).
    Forced,
}

impl AgentReason {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentReason::Declared => "packageManager",
            AgentReason::Lockfile(file) => file,
            AgentReason::Assumed => "assumed",
            AgentReason::Forced => "--agent",
        }
    }
}

/// Lockfiles, most specific first.
///
/// Order is precedence, and it matters: a repo that moved from npm to pnpm
/// routinely still has a stale `package-lock.json` sitting next to its
/// `pnpm-lock.yaml`. The npm lockfile is the one that gets left behind, never
/// the other way round, so the more specific manager wins the tie — and the
/// reason is printed either way, so a wrong guess is visible rather than silent.
const LOCKFILES: &[(&str, Agent)] = &[
    ("pnpm-lock.yaml", Agent::Pnpm),
    ("bun.lockb", Agent::Bun),
    ("bun.lock", Agent::Bun),
    ("yarn.lock", Agent::Yarn),
    ("package-lock.json", Agent::Npm),
    ("npm-shrinkwrap.json", Agent::Npm),
];

/// Work out which package manager a project expects.
///
/// `packageManager` in package.json is Corepack's declaration — an explicit
/// statement of intent, so it wins outright. Otherwise the lockfile in the tree
/// is the evidence. Failing both, npm: it ships with Node, so it's the only
/// agent that can be assumed present.
pub fn detect_agent(project_dir: &Path) -> AgentChoice {
    let declared = package_manager_field(project_dir);
    let (agent, reason) = match declared {
        Some((agent, _)) => (agent, AgentReason::Declared),
        None => LOCKFILES
            .iter()
            .find(|(file, _)| project_dir.join(file).is_file())
            .map(|(file, agent)| (*agent, AgentReason::Lockfile(file)))
            .unwrap_or((Agent::Npm, AgentReason::Assumed)),
    };
    let berry = agent == Agent::Yarn
        && yarn_is_berry(project_dir, declared.and_then(|(_, major)| major));
    AgentChoice { agent, reason, berry }
}

/// The arguments that install a project's dependencies from its lockfile.
///
/// `frozen` is the CI-shaped install: fail rather than update the lockfile. Each
/// agent spells it differently, and getting it wrong isn't cosmetic — classic
/// Yarn's `--frozen-lockfile` is an *unknown option* to Yarn 2+, so the run dies
/// on a flag rather than doing the work.
pub fn install_args(choice: &AgentChoice, frozen: bool) -> Vec<String> {
    let words: &[&str] = match (choice.agent, frozen) {
        (Agent::Npm, false) => &["install"],
        // npm's frozen install is a different verb, not a flag.
        (Agent::Npm, true) => &["ci"],
        (Agent::Yarn, true) if choice.berry => &["install", "--immutable"],
        (_, true) => &["install", "--frozen-lockfile"],
        (_, false) => &["install"],
    };
    words.iter().map(|w| w.to_string()).collect()
}

/// The arguments that run a package.json script. All four agents spell this the
/// same way, and the explicit `run` avoids the shadowing traps of the bare form
/// (`yarn build` would happily run a *binary* named build).
pub fn run_args(script: &str, extra: &[String]) -> Vec<String> {
    let mut args = vec!["run".to_string(), script.to_string()];
    args.extend(extra.iter().cloned());
    args
}

/// The script names a project defines in package.json. Empty when the file is
/// missing, unparseable, or has no `scripts` block — a project you can't read
/// scripts from is one with no scripts to offer.
pub fn read_scripts(project_dir: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(project_dir.join("package.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    value
        .get("scripts")
        .and_then(|s| s.as_object())
        .map(|scripts| scripts.keys().cloned().collect())
        .unwrap_or_default()
}

/// Read `packageManager` (Corepack's `"pnpm@9.1.0"`) as an agent plus its major
/// version, which is how we tell Yarn 1 from Yarn 2+ when it's declared.
fn package_manager_field(project_dir: &Path) -> Option<(Agent, Option<u32>)> {
    let text = std::fs::read_to_string(project_dir.join("package.json")).ok()?;
    let value = json_string_value(&text, "\"packageManager\"")?;
    // `pnpm@9.1.0` / `yarn@3.6.4+sha224.…` / bare `npm`.
    let (name, rest) = value.split_once('@').unwrap_or((value.as_str(), ""));
    let agent = Agent::parse(name)?;
    let major = rest.split('.').next().and_then(|m| m.parse().ok());
    Some((agent, major))
}

/// Is this a Yarn 2+ ("berry") project? The declared major settles it; otherwise
/// `.yarnrc.yml` is berry-only, and berry's lockfile carries a `__metadata` block
/// classic Yarn never writes.
fn yarn_is_berry(project_dir: &Path, declared_major: Option<u32>) -> bool {
    if let Some(major) = declared_major {
        return major >= 2;
    }
    if project_dir.join(".yarnrc.yml").is_file() {
        return true;
    }
    // The marker sits in the lockfile's preamble, so a prefix is enough — these
    // files run to megabytes.
    let Ok(mut file) = std::fs::File::open(project_dir.join("yarn.lock")) else {
        return false;
    };
    use std::io::Read;
    let mut head = [0u8; 512];
    let read = file.read(&mut head).unwrap_or(0);
    String::from_utf8_lossy(&head[..read]).contains("__metadata")
}

/// Pull a top-level string value out of JSON text by key, without taking on a
/// JSON dependency — the same scan [`engines_node`] uses, hoisted.
fn json_string_value(text: &str, quoted_key: &str) -> Option<String> {
    let at = text.find(quoted_key)?;
    let after = &text[at + quoted_key.len()..];
    let colon = after.find(':')?;
    let after_colon = &after[colon + 1..];
    let open = after_colon.find('"')?;
    let value = &after_colon[open + 1..];
    let close = value.find('"')?;
    let found = value[..close].trim();
    (!found.is_empty()).then(|| found.to_string())
}

/// A command to spawn: the program and its arguments, ready for `Command::new`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
}

/// Wrap `command args…` so it runs under a project's pinned Node version.
///
/// dpl still doesn't own `node` — it borrows whichever manager the user already
/// has. fnm can exec a command against a version file directly; nvm is a shell
/// function, so it has to be sourced in a shell first. With no manager, nothing
/// pinned, or a pin no manager can act on, the command runs as `PATH` finds it —
/// exactly what the user's own shell would do.
pub fn pinned_invocation(
    manager: Option<Manager>,
    nvm_script: Option<&Path>,
    pin: Option<&Pin>,
    command: &str,
    args: &[String],
) -> Invocation {
    let direct =
        || Invocation { program: command.to_string(), args: args.to_vec() };

    let Some(manager) = manager else { return direct() };

    // A manager's own pin file is authoritative and read in place; package.json
    // is only a hint, so reduce it to a version a manager will accept.
    let switch = match pin {
        None => return direct(),
        Some(Pin { source: PinSource::PackageJson, version }) => match normalize_range(version) {
            Some(v) => Switch::Version(v),
            None => return direct(),
        },
        Some(_) => Switch::VersionFile,
    };

    match manager {
        Manager::Fnm => {
            let mut fnm = vec!["exec".to_string()];
            match switch {
                Switch::VersionFile => fnm.push("--using-file".into()),
                Switch::Version(v) => {
                    fnm.push("--using".into());
                    fnm.push(v);
                }
            }
            fnm.push("--".into());
            fnm.push(command.to_string());
            fnm.extend(args.iter().cloned());
            Invocation { program: "fnm".into(), args: fnm }
        }
        Manager::Nvm => {
            // Without nvm.sh there is nothing to source, so there is no switch.
            let Some(script) = nvm_script else { return direct() };
            // `1>&2`: even under `--silent`, nvm narrates the switch ("Found
            // .nvmrc with version <18>") on stdout. Moving its output to stderr
            // keeps the command's own stdout pipeable while still showing why a
            // switch failed — nvm reports "version v7 is not yet installed"
            // there too, and swallowing it would leave an unexplained failure.
            let use_version = match switch {
                // Bare `nvm use` reads the version file in the working directory.
                Switch::VersionFile => "nvm use --silent 1>&2".to_string(),
                Switch::Version(v) => format!("nvm use --silent {} 1>&2", shell_quote(&v)),
            };
            // `exec` hands the process over, so exit codes and signals are the
            // command's own rather than the wrapping shell's.
            let mut line = format!(
                ". {} && {} && exec {}",
                shell_quote(&script.to_string_lossy()),
                use_version,
                shell_quote(command),
            );
            for arg in args {
                line.push(' ');
                line.push_str(&shell_quote(arg));
            }
            Invocation { program: "bash".into(), args: vec!["-lc".into(), line] }
        }
    }
}

/// How the manager is asked to switch: read the project's version file, or use
/// a version we resolved ourselves.
enum Switch {
    VersionFile,
    Version(String),
}

/// Single-quote a word for `sh`, so paths and arguments with spaces, `$`, or
/// quotes survive the trip through nvm's shell.
fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

fn read_first_line(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// Pull `engines.node` out of `package.json` without a JSON dependency — a small
/// scan for the key and its string value, enough for a version hint.
fn engines_node(project_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(project_dir.join("package.json")).ok()?;
    // Scan from `"engines"` so this finds *that* block's `node`, not some other
    // key of the same name elsewhere in the file.
    let engines_at = text.find("\"engines\"")?;
    json_string_value(&text[engines_at..], "\"node\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_engine_ranges_to_a_major() {
        assert_eq!(normalize_range("^20.11.0"), Some("20".into()));
        assert_eq!(normalize_range(">=18"), Some("18".into()));
        assert_eq!(normalize_range("20.x"), Some("20".into()));
        assert_eq!(normalize_range("18 || 20"), Some("18".into()));
        assert_eq!(normalize_range("lts/*"), None);
    }

    #[test]
    fn reads_engines_node_from_package_json() {
        let dir = std::env::temp_dir().join(format!("dpl-node-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join("package.json"),
            r#"{ "name": "x", "engines": { "node": ">=20.0.0" }, "scripts": {} }"#,
        )
        .unwrap();
        let pin = read_pin(&dir).unwrap();
        assert_eq!(pin.version, ">=20.0.0");
        assert_eq!(pin.source, PinSource::PackageJson);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn pin(version: &str, source: PinSource) -> Pin {
        Pin { version: version.into(), source }
    }

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn without_a_manager_the_command_runs_as_path_finds_it() {
        let inv = pinned_invocation(None, None, Some(&pin("20", PinSource::Nvmrc)), "npm", &args(&["ci"]));
        assert_eq!(inv, Invocation { program: "npm".into(), args: args(&["ci"]) });
    }

    /// Nothing pinned means nothing to switch to — don't ask the manager to
    /// resolve a version the project never named.
    #[test]
    fn an_unpinned_project_runs_the_command_directly() {
        let inv = pinned_invocation(Some(Manager::Fnm), None, None, "npm", &args(&["ci"]));
        assert_eq!(inv, Invocation { program: "npm".into(), args: args(&["ci"]) });
    }

    #[test]
    fn fnm_execs_against_the_projects_version_file() {
        let inv = pinned_invocation(
            Some(Manager::Fnm),
            None,
            Some(&pin("20", PinSource::Nvmrc)),
            "npm",
            &args(&["run", "build"]),
        );
        assert_eq!(inv.program, "fnm");
        assert_eq!(inv.args, args(&["exec", "--using-file", "--", "npm", "run", "build"]));
    }

    /// package.json is a hint, not a pin file — fnm gets the resolved major.
    #[test]
    fn fnm_resolves_a_package_json_range_to_a_version() {
        let inv = pinned_invocation(
            Some(Manager::Fnm),
            None,
            Some(&pin("^20.11.0", PinSource::PackageJson)),
            "npm",
            &args(&["ci"]),
        );
        assert_eq!(inv.args, args(&["exec", "--using", "20", "--", "npm", "ci"]));
    }

    /// `engines.node: "lts/*"` isn't a version we can hand a manager, so the
    /// command runs as-is rather than failing on a bogus `--using`.
    #[test]
    fn an_unresolvable_range_falls_back_to_running_directly() {
        let inv = pinned_invocation(
            Some(Manager::Fnm),
            None,
            Some(&pin("lts/*", PinSource::PackageJson)),
            "npm",
            &args(&["ci"]),
        );
        assert_eq!(inv, Invocation { program: "npm".into(), args: args(&["ci"]) });
    }

    #[test]
    fn nvm_sources_its_script_then_execs_the_command() {
        let script = std::path::PathBuf::from("/Users/x/.nvm/nvm.sh");
        let inv = pinned_invocation(
            Some(Manager::Nvm),
            Some(&script),
            Some(&pin("20", PinSource::Nvmrc)),
            "npm",
            &args(&["ci"]),
        );
        assert_eq!(inv.program, "bash");
        assert_eq!(inv.args[0], "-lc");
        assert_eq!(
            inv.args[1],
            ". '/Users/x/.nvm/nvm.sh' && nvm use --silent 1>&2 && exec 'npm' 'ci'"
        );
    }

    #[test]
    fn nvm_without_its_script_runs_the_command_directly() {
        let inv =
            pinned_invocation(Some(Manager::Nvm), None, Some(&pin("20", PinSource::Nvmrc)), "npm", &args(&["ci"]));
        assert_eq!(inv, Invocation { program: "npm".into(), args: args(&["ci"]) });
    }

    /// Arguments reach npm intact, however hostile they are to a shell.
    #[test]
    fn nvm_quotes_arguments_and_paths() {
        let script = std::path::PathBuf::from("/home/o'brien/my nvm/nvm.sh");
        let inv = pinned_invocation(
            Some(Manager::Nvm),
            Some(&script),
            Some(&pin("18", PinSource::PackageJson)),
            "npm",
            &args(&["run", "build -- --mode=$PROD"]),
        );
        assert_eq!(
            inv.args[1],
            r#". '/home/o'\''brien/my nvm/nvm.sh' && nvm use --silent '18' 1>&2 && exec 'npm' 'run' 'build -- --mode=$PROD'"#
        );
    }

    /// A scratch project directory, unique per test.
    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dpl-agent-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn a_bare_project_is_assumed_to_be_npm() {
        let dir = scratch("bare");
        write(&dir, "package.json", r#"{ "name": "x" }"#);
        let choice = detect_agent(&dir);
        assert_eq!(choice.agent, Agent::Npm);
        assert_eq!(choice.reason, AgentReason::Assumed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_lockfile_identifies_its_agent() {
        for (file, expected) in
            [("pnpm-lock.yaml", Agent::Pnpm), ("yarn.lock", Agent::Yarn), ("bun.lockb", Agent::Bun), ("package-lock.json", Agent::Npm)]
        {
            let dir = scratch(&file.replace('.', "-"));
            write(&dir, "package.json", r#"{ "name": "x" }"#);
            write(&dir, file, "");
            let choice = detect_agent(&dir);
            assert_eq!(choice.agent, expected, "{file}");
            assert_eq!(choice.reason, AgentReason::Lockfile(file));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The npm lockfile is what gets left behind by a migration, so the more
    /// specific manager wins rather than npm clobbering a pnpm store.
    #[test]
    fn a_stale_npm_lockfile_does_not_beat_pnpm() {
        let dir = scratch("stale");
        write(&dir, "package.json", r#"{ "name": "x" }"#);
        write(&dir, "package-lock.json", "{}");
        write(&dir, "pnpm-lock.yaml", "");
        assert_eq!(detect_agent(&dir).agent, Agent::Pnpm);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Corepack's declaration is intent, not evidence — it beats any lockfile.
    #[test]
    fn the_package_manager_field_wins_over_a_lockfile() {
        let dir = scratch("declared");
        write(&dir, "package.json", r#"{ "packageManager": "pnpm@9.1.0" }"#);
        write(&dir, "yarn.lock", "");
        let choice = detect_agent(&dir);
        assert_eq!(choice.agent, Agent::Pnpm);
        assert_eq!(choice.reason, AgentReason::Declared);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn yarn_1_and_yarn_berry_are_told_apart() {
        // Declared major.
        let dir = scratch("yarn-declared");
        write(&dir, "package.json", r#"{ "packageManager": "yarn@3.6.4" }"#);
        assert!(detect_agent(&dir).berry);
        write(&dir, "package.json", r#"{ "packageManager": "yarn@1.22.19" }"#);
        assert!(!detect_agent(&dir).berry);
        let _ = std::fs::remove_dir_all(&dir);

        // Undeclared: .yarnrc.yml is berry-only.
        let dir = scratch("yarn-rc");
        write(&dir, "package.json", r#"{ "name": "x" }"#);
        write(&dir, "yarn.lock", "");
        write(&dir, ".yarnrc.yml", "nodeLinker: node-modules\n");
        assert!(detect_agent(&dir).berry);
        let _ = std::fs::remove_dir_all(&dir);

        // Undeclared: berry's lockfile preamble gives it away.
        let dir = scratch("yarn-lock");
        write(&dir, "package.json", r#"{ "name": "x" }"#);
        write(&dir, "yarn.lock", "# This file is generated by running \"yarn install\"\n\n__metadata:\n  version: 8\n");
        assert!(detect_agent(&dir).berry);
        let _ = std::fs::remove_dir_all(&dir);

        // Classic yarn.lock has no such block.
        let dir = scratch("yarn-classic");
        write(&dir, "package.json", r#"{ "name": "x" }"#);
        write(&dir, "yarn.lock", "# yarn lockfile v1\n\n\nfoo@^1.0.0:\n  version \"1.0.0\"\n");
        assert!(!detect_agent(&dir).berry);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_agent_spells_a_frozen_install_its_own_way() {
        let choice = |agent, berry| AgentChoice { agent, reason: AgentReason::Assumed, berry };

        assert_eq!(install_args(&choice(Agent::Npm, false), false), args(&["install"]));
        assert_eq!(install_args(&choice(Agent::Npm, false), true), args(&["ci"]));
        assert_eq!(install_args(&choice(Agent::Pnpm, false), true), args(&["install", "--frozen-lockfile"]));
        assert_eq!(install_args(&choice(Agent::Bun, false), true), args(&["install", "--frozen-lockfile"]));
        assert_eq!(install_args(&choice(Agent::Yarn, false), true), args(&["install", "--frozen-lockfile"]));
        // Berry rejects --frozen-lockfile outright.
        assert_eq!(install_args(&choice(Agent::Yarn, true), true), args(&["install", "--immutable"]));
        assert_eq!(install_args(&choice(Agent::Yarn, true), false), args(&["install"]));
    }

    #[test]
    fn scripts_run_through_the_explicit_run_verb() {
        assert_eq!(run_args("build", &[]), args(&["run", "build"]));
        assert_eq!(run_args("test", &args(&["--watch"])), args(&["run", "test", "--watch"]));
    }

    #[test]
    fn agent_names_round_trip() {
        for agent in Agent::ALL {
            assert_eq!(Agent::parse(agent.as_str()), Some(agent));
        }
        assert_eq!(Agent::parse("PNPM"), Some(Agent::Pnpm));
        assert_eq!(Agent::parse("deno"), None);
    }

    #[test]
    fn nvmrc_wins_over_package_json() {
        let dir = std::env::temp_dir().join(format!("dpl-node-test2-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("package.json"), r#"{ "engines": { "node": "18" } }"#).unwrap();
        write_nvmrc(&dir, "20").unwrap();
        let pin = read_pin(&dir).unwrap();
        assert_eq!(pin.version, "20");
        assert_eq!(pin.source, PinSource::Nvmrc);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
