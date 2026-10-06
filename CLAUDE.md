# sshh

SSH connection manager in Rust: TUI (ratatui + crossterm, keyboard and mouse) and `ssh` wrapper.
Used as `sshh [ssh args]` (optionally `alias ssh=sshh`).

## Principles
- **Never reimplement SSH**: always `exec` the system `ssh` (`connect::exec_ssh`).
  sshh must never get in the way of a connection: if the parser or the DB fails, warn and pass the
  args through unchanged.
- **No secrets are stored** (passwords): keys and ssh-agent.
- Source of truth: SQLite at `~/.local/share/sshh/sshh.db` (`$SSHH_DB` to override).
  JSON import/export (format = `model::HostData`).
- Ecosystem compatibility: `include.rs` generates `~/.ssh/config.d/sshh.conf` (enabled if the file
  exists; `include::refresh` after every DB change). **`~/.ssh/config` is never modified**: the user
  adds `Include config.d/sshh.conf` at the top by hand (after a `Host` it would be conditional to
  that block).
- Option values: use `model::config_value` (only quotes single-value options; multi-argument ones
  like `LocalForward 8080 host:80` must not be quoted).

## Modules
- `ssh_args`: OpenSSH CLI parser (flags with/without argument, destination, `ssh://`, `--`).
- `connect`: wrapper mode. If the destination is a saved alias, injects `-o Key=Value` before the
  destination only for what the user didn't set. Otherwise resolves the effective destination with
  `ssh -G` (applies ssh_config) and looks it up by `user@hostname:port`; on a match, passes args
  untouched. If it's not saved and the session is interactive, opens the wizard (`SSHH_NO_WIZARD=1`
  disables it). `-G/-V/-O/-Q` (`info_only`) neither open the wizard nor count in the history.
- `db`: SQLite + migrations via `PRAGMA user_version` (append to `MIGRATIONS`, never edit existing ones).
- `tui`: `sshh` without args, and the wizard. `app.rs` = state + events (testable without a terminal);
  side effects (DB, clipboard, $EDITOR) are requested through `App::request` and run by `mod.rs`.
  `form.rs` = add/edit/wizard form. `ui.rs` = rendering. `term.rs` = terminal, $EDITOR and clipboard
  (wl-copy/xclip, OSC 52 fallback). Connecting leaves the TUI and calls `connect::run`.
- Shortcuts: vim/lazygit style (`j/k`, `gg/G`, `Ctrl-d/u/f/b`, `/`, `a`, `e`, `t`, `dd`, `yy`, `s`, `c`,
  `o`, `R`, `Tab`, `?`). `Esc` always goes back one level; only `q`/`Ctrl-c` quit.
- `cli`: own subcommands (`ls`, `add`, `rm`, `history`, `export`, `import`, `import-ssh-config`,
  `ssh-config`); their names are in `model::RESERVED_ALIASES` (add any new subcommand there).
- `ssh_config`: ssh_config parser to import concrete `Host`s (follows `Include`, ignores wildcards,
  `Match` and the generated file `GENERATED_FILE_NAME`).
- Imports: `Db::import_hosts` in one transaction; `--dry-run` = same transaction without commit.
  JSON format = `model::HostData` (empty fields omitted when serializing).

## Development
- `cargo test`, `cargo clippy --all-targets`.
- Manual tests without connecting: `SSHH_DB=/tmp/x.db SSHH_SSH=<script printing its args> sshh ...`.
  Set these per command; don't `export` them in the user's shell.
- `SSHH_DEBUG=1` prints to stderr how the destination is resolved and the command that runs.
- The TUI can be tested with tmux: `tmux new -d -s t -x 120 -y 30 sshh` + `send-keys` / `capture-pane -p`.
- `SSHH_INCLUDE_FILE` redirects the generated file; to test `include_state` use `HOME=<tmp>`
  (note: the real `ssh` reads the home from /etc/passwd, not `$HOME`; use `ssh -F <config>`).
- Language: English for UI messages, comments, docs and tests. Quote values with single quotes
  (`'web'`) in messages.

## Roadmap
1. ✅ Base: DB, migrations, ssh parser, exec.
2. ✅ TUI: list, details, fuzzy search, tag filter, mouse.
3. ✅ Wizard for new connections, CRUD in the TUI, tags, copy command.
4. ✅ History, JSON import/export, `~/.ssh/config` import.
5. ✅ Generated ssh_config file; quick actions (sftp, ssh-copy-id).
