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
    // Rust ignora SIGPIPE por defecto y `println!` haría panic con
    // `sshh ls | head`. Se restaura el comportamiento normal de Unix.
    // SAFETY: se llama al arrancar, antes de crear hilos.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sshh: {e:#}");
            // Mismo código que usa ssh para sus propios errores.
            ExitCode::from(255)
        }
    }
}

fn run() -> anyhow::Result<()> {
    let raw: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some(args) = raw.iter().map(|a| a.to_str().map(String::from)).collect::<Option<Vec<_>>>() else {
        // Argumentos que no son UTF-8: no los interpretamos, se los damos a ssh.
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
