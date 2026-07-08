//! `dpl` — the command-line entry point. Parses the tree in [`cli`], then
//! either round-trips a control verb to the `dpld` daemon or dispatches into
//! the `dpl dply …` platform commands.

mod cli;
mod commands;
mod daemon;
mod output;

use anyhow::Result;
use clap::Parser;
use dpl_core::ipc::{Request, Response};

use cli::{Cli, Command};

fn main() {
    if let Err(e) = run() {
        // Print the whole error chain, most-specific last.
        eprintln!("error: {e}");
        for cause in e.chain().skip(1) {
            eprintln!("  caused by: {cause}");
        }
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Cli::parse();
    let host = args.host.as_deref();
    let home = args.config_dir.clone();

    match args.command {
        Command::Ping => daemon_ping(home.as_deref()),
        Command::Status => daemon_status(home.as_deref()),

        // Local .test site management (talks to dpld).
        Command::Sites => commands::local::sites(home.as_deref(), args.json),
        Command::Park { path } => commands::local::park(home.as_deref(), path),
        Command::Unpark { path } => commands::local::unpark(home.as_deref(), path),
        Command::Link { path, name } => commands::local::link(home.as_deref(), path, name),
        Command::Unlink { name } => commands::local::unlink(home.as_deref(), name),
        Command::Secure { name } => commands::local::secure(home.as_deref(), name, true),
        Command::Unsecure { name } => commands::local::secure(home.as_deref(), name, false),
        Command::Open { name } => commands::local::open(home.as_deref(), name),
        Command::Php => commands::local::php_list(args.json),
        Command::Use { version, name, default } => {
            commands::local::use_php(home.as_deref(), version, name, default)
        }
        Command::Logs { name, lines, follow } => {
            commands::local::logs(home.as_deref(), name, lines, follow)
        }
        Command::Share { name } => commands::local::share(home.as_deref(), name),
        Command::Restart => commands::local::restart(home.as_deref()),
        Command::Paths => commands::local::paths(home.as_deref()),
        Command::Doctor => commands::local::doctor(home.as_deref()),
        Command::Setup { no_ports } => commands::local::setup(home.as_deref(), !no_ports),
        Command::Unsetup => commands::local::unsetup(home.as_deref()),
        Command::Trust => commands::local::trust(home.as_deref(), true),
        Command::Untrust => commands::local::trust(home.as_deref(), false),
        Command::Services => commands::local::services(home.as_deref(), args.json),
        Command::Service { action, name, engine, version, port } => {
            commands::local::service(home.as_deref(), action, name, engine, version, port, args.json)
        }
        Command::Db { action, name, engine, port, file } => {
            commands::local::db(home.as_deref(), action, engine, name, port, file)
        }
        Command::Mail { action, id } => commands::local::mail(home.as_deref(), action, id, args.json),
        Command::Daemon { action } => commands::daemon::manage(home.as_deref(), action),
        Command::Tld { action, name } => commands::local::tld(home.as_deref(), action, name, args.json),
        Command::Version => {
            println!("dpl {}", env!("CARGO_PKG_VERSION"));
            // Best-effort: also report the daemon's version if it's up.
            if let Ok(Response::Version { version }) =
                daemon::call(Request::Version, home.as_deref())
            {
                println!("dpld {version}");
            } else {
                println!("dpld (not running)");
            }
            Ok(())
        }

        // Top-level auth aliases delegate into the dply auth handlers.
        Command::Login(a) => commands::dply::auth::login(host, home, a.no_browser),
        Command::Logout => commands::dply::auth::logout(host, home),
        Command::Whoami => commands::dply::auth::whoami(host, home, args.json),

        Command::Dply(cmd) => commands::dply::run(cmd, host, home, args.json),
    }
}

fn daemon_ping(home: Option<&str>) -> Result<()> {
    match daemon::call(Request::Ping, home)? {
        Response::Pong => {
            println!("pong");
            Ok(())
        }
        other => unexpected(other),
    }
}

fn daemon_status(home: Option<&str>) -> Result<()> {
    match daemon::call(Request::Status, home)? {
        Response::Status { status } => {
            println!("dpld {}", status.version);
            println!("uptime      {}s", status.uptime_secs);
            println!("proxy       {}", if status.proxy_running { "running" } else { "stopped" });
            println!("sites       {}", status.site_count);
            Ok(())
        }
        other => unexpected(other),
    }
}

fn unexpected(resp: Response) -> Result<()> {
    match resp {
        Response::Error { message } => anyhow::bail!("daemon error: {message}"),
        other => anyhow::bail!("unexpected daemon response: {other:?}"),
    }
}
