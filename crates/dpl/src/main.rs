//! `dpl` — the command-line entry point. Parses the tree in [`cli`], then
//! either round-trips a control verb to the `dpld` daemon or dispatches into
//! the `dpl dply …` platform commands.

mod checks;
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
        Command::Proxy { name, target } => commands::local::proxy_set(home.as_deref(), name, target),
        Command::Unproxy { name } => commands::local::proxy_remove(home.as_deref(), name),
        Command::Proxies => commands::local::proxies(home.as_deref(), args.json),
        Command::Php { cmd } => match cmd {
            None => commands::local::php_list_home(home.as_deref(), args.json),
            Some(cli::PhpCmd::Available) => commands::local::php_available(args.json),
            Some(cli::PhpCmd::Install { version }) => commands::local::php_install(&version),
            Some(cli::PhpCmd::Upgrade { version }) => commands::local::php_upgrade(&version),
            Some(cli::PhpCmd::Uninstall { version }) => commands::local::php_uninstall(&version),
            Some(cli::PhpCmd::Repair { version }) => commands::local::php_repair(&version),
            Some(cli::PhpCmd::Fix { version }) => commands::local::php_fix(&version),
            Some(cli::PhpCmd::ExtAvailable { version }) => commands::local::php_ext_available(&version, args.json),
            Some(cli::PhpCmd::ExtInstall { version, name }) => commands::local::php_ext_install(&version, &name),
            Some(cli::PhpCmd::ExtUninstall { version, name }) => commands::local::php_ext_uninstall(&version, &name),
        },
        Command::Valet { cmd } => match cmd {
            cli::ValetCmd::List => commands::valet::list(args.json),
            cli::ValetCmd::Import { links_only, parks_only, match_tld, manifest } => {
                let parks = !links_only;
                let links = !parks_only;
                commands::valet::import(home.as_deref(), parks, links, match_tld, manifest.as_deref())
            }
            cli::ValetCmd::Remove { manifest } => {
                commands::valet::remove(home.as_deref(), manifest.as_deref())
            }
        },
        Command::Use { version, name, default } => {
            commands::local::use_php(home.as_deref(), version, name, default)
        }
        Command::Logs { name, lines, follow } => {
            commands::local::logs(home.as_deref(), name, lines, follow)
        }
        Command::Tinker { site } => commands::local::tinker(home.as_deref(), site),
        Command::Share { name } => commands::local::share(home.as_deref(), name),
        Command::Start => commands::daemon::manage(home.as_deref(), "start".into()),
        Command::Stop => commands::daemon::manage(home.as_deref(), "stop".into()),
        Command::Reload => commands::local::restart(home.as_deref()),
        Command::Restart => commands::local::restart(home.as_deref()),
        Command::Repair => commands::local::repair_backends(home.as_deref()),
        Command::Paths => commands::local::paths(home.as_deref()),
        Command::Doctor => commands::doctor::run(home.as_deref(), args.json),
        Command::Parity { site, remote } => {
            commands::parity::run(home.as_deref(), args.host.as_deref(), site, remote, args.json)
        }
        Command::Setup { no_ports, as_user } => {
            commands::local::setup(home.as_deref(), !no_ports, as_user.as_deref())
        }
        Command::Runtime { site, runtime } => commands::local::set_runtime(home.as_deref(), site, runtime),
        Command::Octane { site, server } => commands::local::octane_setup(home.as_deref(), &site, &server),
        Command::Profile { command } => match command {
            None | Some(cli::ProfileCmd::Status) => {
                commands::profile::status(home.as_deref(), args.json)
            }
            Some(cli::ProfileCmd::On { site }) => {
                commands::profile::set(home.as_deref(), site, true)
            }
            Some(cli::ProfileCmd::Off { site }) => {
                commands::profile::set(home.as_deref(), site, false)
            }
            Some(cli::ProfileCmd::Open { site }) => commands::profile::open(home.as_deref(), site),
        },
        Command::Preload { command } => match command {
            None | Some(cli::PreloadCmd::Status) => {
                commands::preload::status(home.as_deref(), args.json)
            }
            Some(cli::PreloadCmd::Generate { site }) => {
                commands::preload::generate(home.as_deref(), site)
            }
            Some(cli::PreloadCmd::On { site, script }) => {
                commands::preload::set(home.as_deref(), site, Some(script))
            }
            Some(cli::PreloadCmd::Off { site }) => {
                commands::preload::set(home.as_deref(), site, None)
            }
        },
        Command::Node { command } => match command {
            None | Some(cli::NodeCmd::Status) => commands::node::status(home.as_deref(), args.json),
            Some(cli::NodeCmd::Use { version, site }) => {
                commands::node::use_version(home.as_deref(), version, site)
            }
            Some(cli::NodeCmd::Install { version }) => commands::node::install(&version),
            Some(cli::NodeCmd::Detect { site }) => commands::node::detect(home.as_deref(), site),
        },
        Command::Xdebug { command } => match command {
            None | Some(cli::XdebugCmd::Status) => {
                commands::xdebug::status(home.as_deref(), args.json)
            }
            Some(cli::XdebugCmd::On { site }) => {
                commands::xdebug::set_mode(home.as_deref(), "debug", site)
            }
            Some(cli::XdebugCmd::Off { site }) => {
                commands::xdebug::set_mode(home.as_deref(), "off", site)
            }
            Some(cli::XdebugCmd::Mode { mode, site }) => {
                commands::xdebug::set_mode(home.as_deref(), &mode, site)
            }
            Some(cli::XdebugCmd::Port { port }) => commands::xdebug::set_port(home.as_deref(), port),
            Some(cli::XdebugCmd::Ide { key }) => commands::xdebug::set_ide(home.as_deref(), key),
        },
        Command::Resolution { mode } => commands::local::resolution(home.as_deref(), mode),
        Command::Takeover => commands::local::takeover(home.as_deref()),
        Command::Untakeover => commands::local::untakeover(home.as_deref()),
        Command::Unsetup => commands::local::unsetup(home.as_deref()),
        Command::Trust => commands::local::trust(home.as_deref(), true),
        Command::Untrust => commands::local::trust(home.as_deref(), false),
        Command::Services => commands::local::services(home.as_deref(), args.json),
        Command::Service { action, name, engine, version, port } => {
            commands::local::service(home.as_deref(), action, name, engine, version, port, args.json)
        }
        Command::Up { path, save } => commands::local::up(home.as_deref(), path, save),
        Command::Db { action, name, branch, engine, port, file, database } => {
            commands::local::db(home.as_deref(), action, engine, name, branch, port, file, database)
        }
        Command::Mail { command } => {
            use cli::MailCmd;
            let home = home.as_deref();
            match command.unwrap_or(MailCmd::List { mailbox: None, search: None }) {
                MailCmd::List { mailbox, search } => {
                    commands::mail::list(home, mailbox, search, args.json)
                }
                MailCmd::Mailboxes => commands::mail::mailboxes(home, args.json),
                MailCmd::Show { id, part } => {
                    commands::mail::show(home, &id, part.parse()?, args.json)
                }
                MailCmd::Attachments { id } => commands::mail::attachments(home, &id, args.json),
                MailCmd::Save { id, index, out } => commands::mail::save(home, &id, index, out),
                MailCmd::Links { id } => commands::mail::links(home, &id, args.json),
                MailCmd::Send { to, from, subject, mailbox, html, body } => {
                    commands::mail::send(to, from, subject, mailbox, html, body)
                }
                MailCmd::Clear { mailbox } => commands::mail::clear(home, mailbox),
            }
        }
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
        Command::Keel(cmd) => commands::keel::run(cmd, home, args.json),
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
