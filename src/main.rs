mod cli;
mod connect;
mod db;
mod include;
mod model;
mod ssh_args;
mod ssh_config;
mod tui;

use std::ffi::OsString;
use std::io::IsTerminal;
use std::process::ExitCode;

fn main() -> ExitCode {
    // Rust ignores SIGPIPE by default, so `println!` would panic on
    // `sshh ls | head`. Restore the usual Unix behaviour.
    // SAFETY: called at startup, before any thread is spawned.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sshh: {e:#}");
            // Same exit code ssh uses for its own errors.
            ExitCode::from(255)
        }
    }
}

fn run() -> anyhow::Result<()> {
    let raw: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some(args) = raw.iter().map(|a| a.to_str().map(String::from)).collect::<Option<Vec<_>>>() else {
        // Non UTF-8 arguments: don't interpret them, hand them to ssh.
        match connect::exec_ssh(&raw)? {}
    };

    if args.is_empty() {
        return if std::io::stdout().is_terminal() { tui::run() } else { cli::list() };
    }
    if cli::is_own_command(&args) {
        return cli::run(args);
    }
    match connect::run(args)? {}
}
