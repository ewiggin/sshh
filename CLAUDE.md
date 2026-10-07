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
- `tui`: `sshh` without args, and the wizard. Lazygit-style layout: search + list + details on the
  left, terminal columns on the right (`App::columns` = connection ids left to right,
  `active_column`, `zoomed`). The list is "column 0" for Alt-h/l. Requests that target a session carry
  its id (`Input`, `Paste`, `Scroll`); the loop resizes each session to its `App::column_areas` rect.
  `app.rs` = state + events (testable without a terminal); side effects (DB, clipboard, $EDITOR,
  sessions) are requested through `App::request` and run by `mod.rs`, which owns the sessions and
  publishes their state in `App::sessions`. `session.rs` = ssh in a PTY (portable-pty) + vt100
  parser, rendered with tui-term; `key_bytes` encodes keys as xterm does. The loop polls crossterm
  with a short timeout and redraws when a session reader thread sets `dirty`.
  `form.rs` = add/edit/wizard form. `ui.rs` = rendering. `term.rs` = terminal, $EDITOR, external
  programs (`run_external` suspends/resumes the TUI; never `exec` from the TUI or sessions die) and
  clipboard (wl-copy/xclip, OSC 52 fallback).
- Shortcuts: vim/lazygit style. `Space` connects (shows the selection in the active column), `Enter`
  edits (like `e`); in the search box the list filters while typing and `Enter`/`Esc` go back.
  Others: `j/k`, `gg/G`, `Ctrl-d/u/f/b`, `/`, `a`, `e`, `t`, `dd`, `yy`, `s`, `c`,
  `f`, `x`, `o`, `R`, `Tab`, `?`). `Tab`/`Shift-Tab` cycle list → details → active column. Columns: `Alt-v` split, `Alt-h/l` or `Alt-←/→` move,
  `Alt-1..9` jump, `Alt-j/k` change the column's session, `Alt-w` close column, `Alt-f`/`Alt-z` full
  screen (zoom), `Alt-H/L` move column. `Alt-f` was the user's choice despite clashing with readline;
  avoid taking more readline Alt keys (`Alt-.`, `b`, `d`, `<`, `>`).
  With the terminal focused every other key goes to ssh. `Esc` always goes back one level (except in
  the terminal); only `q`/`Ctrl-c` quit (confirming if sessions are alive).
- `cli`: own subcommands (`ls`, `add`, `rm`, `history`, `export`, `import`, `import-ssh-config`,
  `ssh-config`); their names are in `model::RESERVED_ALIASES` (add any new subcommand there).
- `ssh_config`: ssh_config parser to import concrete `Host`s (follows `Include`, ignores wildcards,
  `Match` and the generated file `GENERATED_FILE_NAME`).
- Imports: `Db::import_hosts` in one transaction; `--dry-run` = same transaction without commit.
  JSON format = `model::HostData` (empty fields omitted when serializing).

## Development
- `cargo test`, `cargo clippy --all-targets`, `make lint-man`.
- Docs to keep in sync when changing subcommands, options or shortcuts: `README.md`, the TUI help
  (`ui::draw_help`, footer hints) and the man page `man/sshh.1` (roff, written by hand).
- Static build: `make static` / `make install-static` / `make dist` (musl target
  `x86_64-unknown-linux-musl`, needs `musl-gcc` for bundled SQLite; tests also pass with
  `cargo test --target x86_64-unknown-linux-musl`). Release profile has `strip = true`.
- Install: `make && make install` (PREFIX defaults to `~/.local`; `install` never builds), or
  `install.sh` (POSIX sh, used as `curl … | sh`): checks requirements, clones to a temp dir (or uses
  the clone it runs from), runs `make build` and `make install`, using sudo only for the copy.
  Test it with `SSHH_REPO=file://… SSHH_PREFIX=<tmp>`; keep it `sh`/dash compatible.
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
6. ✅ Embedded ssh sessions (several at once) next to the list.
7. ✅ Terminal columns side by side.
