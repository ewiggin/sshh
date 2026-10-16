# sshh

SSH connection manager in Rust: TUI (ratatui + crossterm, keyboard and mouse) and `ssh` wrapper.
Used as `sshh [ssh args]` (optionally `alias ssh=sshh`).

## Principles
- **Never reimplement SSH**: always `exec` the system `ssh` (`connect::exec_ssh`).
  sshh must never get in the way of a connection: if the parser or the DB fails, warn and pass the
  args through unchanged.
- **No secrets are stored** (passwords): keys and ssh-agent.
- Source of truth: SQLite at `~/.local/share/sshh/sshh.db` (`$XDG_DATA_HOME` respected, same path
  on macOS, not `~/Library`; `$SSHH_DB` to override).
  JSON import/export (format = `model::HostData`).
- Ecosystem compatibility: `include.rs` generates `~/.ssh/config.d/sshh.conf` (enabled if the file
  exists; `include::refresh` after every DB change). **`~/.ssh/config` is never modified**: the user
  adds `Include config.d/sshh.conf` at the top by hand (after a `Host` it would be conditional to
  that block).
- Colors: only the 16 ANSI colors + default fg/bg + modifiers (no `Color::Indexed`/`Rgb`), so the UI
  follows the terminal theme (Omarchy themes set the palette; color4/blue = accent).
- Option values: use `model::config_value` (only quotes single-value options; multi-argument ones
  like `LocalForward 8080 host:80` must not be quoted).

## Modules
- `ssh_args`: OpenSSH CLI parser (flags with/without argument, destination, `ssh://`, `--`).
- `connect`: wrapper mode. If the destination is a saved alias, injects `-o Key=Value` before the
  destination only for what the user didn't set. Otherwise resolves the effective destination with
  `ssh -G` (applies ssh_config) and looks it up by `user@hostname:port`; on a match, passes args
  untouched. If it's not saved and the session is interactive, opens the wizard (`SSHH_NO_WIZARD=1`
  disables it). `-G/-V/-O/-Q` (`info_only`) neither open the wizard nor count in the history.
  `HostData::tmux_session`: `with_tmux` adds `-t` + `model::tmux_command` (remote
  `tmux new-session -A`) only for interactive shells (`SshInvocation::interactive_shell`, a tty in
  wrapper mode); never in tool_args (sftp, ssh-copy-id) nor in the generated include file.
- `db`: SQLite + migrations via `PRAGMA user_version` (append to `MIGRATIONS`, never edit existing ones).
- `tui`: `sshh` without args, and the wizard. Lazygit-style layout: search + list + details on the
  left, terminal columns on the right. `App::columns: Vec<Vec<SessionKey>>` = columns left to right,
  each a stack of panes top to bottom; `active_column`/`active_row`, `pane_areas` (by `Pane` =
  (column, row)). Each `SessionKey` is a connection
  id + `SessionKind` (Ssh / Sftp): a connection can have both; `App::host_session` aggregates them,
  `zoomed` = full screen). The list is "column 0" for Alt-h/l.
  Sizes: left side = width / (columns + 2), at least the old 24% (24..40), and `App::columns_room`
  (width with the list at that minimum) is what `max_columns` uses; under `ui::PORTRAIT_WIDTH` `App::portrait` (list on top, panes
  below, limited by `stacked_capacity`); `App::stacked` = panes drawn one under the other (portrait
  or columns narrower than `MIN_COLUMN_WIDTH`), Alt-j/k then walk them all (`focus_stacked`). Requests that target a session carry
  its id (`Input`, `Paste`, `Scroll`); the loop resizes each session to its `App::pane_areas` rect.
  `app.rs` = state + events (testable without a terminal); side effects (DB, clipboard, $EDITOR,
  sessions) are requested through `App::request` and run by `mod.rs`, which owns the sessions and
  publishes their state in `App::sessions`. `session.rs` = ssh in a PTY (portable-pty) + vt100
  parser, rendered with tui-term; `key_bytes` encodes keys as xterm does. `Responder` (vt100
  callbacks) answers terminal queries (DA1/DA2, DSR/CPR, XTVERSION, DECRQM, 18t) from the reader
  thread. `mouse_bytes` encodes mouse events for programs in mouse mode (`App::mouse_sessions`); otherwise
  dragging over a pane selects text (`App::selection`, screen cells) and releasing sends
  `Request::CopySelection` (`Session::text_between`).
  History mode (`CopyMode`, `Alt-s`): snapshot of history+screen, line cursor/selection, search;
  keys routed via `Request::HistoryKey` while the id is in `App::history_sessions`. The loop polls crossterm
  with a short timeout and redraws when a session reader thread sets `dirty`.
  `form.rs` = add/edit/wizard form. `ui.rs` = rendering. `term.rs` = terminal, $EDITOR, external
  programs (`run_external` suspends/resumes the TUI; never `exec` from the TUI or sessions die) and
  clipboard (pbcopy on macOS, wl-copy/xclip, OSC 52 fallback).
- List pane tabs (`App::tab`, `ListTab`): Connections, Tags and Layouts (`[`/`]`, click on the title via
  `App::tab_areas`). Tags tab = `App::tags` (name + count, recomputed in `refilter`); Space/Enter
  sets the query to `#tag` and goes back; connection actions are ignored there (`on_tags_key`).
  `Alt-v` on a tag (`App::open_tag`) replaces the panes with a grid of the tag's connections and
  asks for the missing sessions with `Request::OpenSessions`. Tags and Layouts share `on_side_key`.
  Layouts tab = `App::layouts` (`db::Layout`, tables `layouts` + `layout_panes`, which cascade when
  a connection is deleted): `a` saves the panes (`Mode::LayoutName` → `Request::SaveLayout`), Space
  restores them (`App::open_layout`, `grid` if they don't fit), `dd` deletes.
- Shortcuts: vim/lazygit style. `Space` connects (shows the selection in the active column), `Enter`
  edits (like `e`); in the search box the list filters while typing and `Enter`/`Esc` go back.
  Others: `j/k`, `gg/G`, `Ctrl-d/u/f/b`, `/`, `a`, `e`, `t`, `dd`, `yy`, `s`, `c`,
  `f`, `x`, `o`, `R`, `Tab`, `?`). `Tab`/`Shift-Tab` cycle list → details → active pane. Panes: `Alt-v` new column, `Alt--` new pane
  below the last column, `Alt-h/j/k/l` (+ arrows) focus like vim, `Alt-1..9` jump to column,
  `Alt-J` stack below the right (or left) column, `Alt-K` unstack, `Alt-H/L` move column, `Alt-n/p`
  change the pane's session, `Alt-w` close pane, `Alt-f`/`Alt-z` full screen (zoom), `Alt-a` broadcast (`App::broadcast`:
  `Input`/`Paste` fan out to `App::input_targets`, the visible live panes of the same kind). `Alt-f` was the user's choice despite clashing with readline;
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
- Workflow for every change: 1) make the change (code + tests + docs above), 2) commit it,
  3) add its release note under `## Unreleased` in `CHANGES.md` (create the section if the last
  release removed it; user-visible changes only) and commit that too.

## Task workflow
Features and bugs are GitHub issues on the project board "sshh" (`gh project … 6 --owner ewiggin`;
Status: LATER, NEXT, NOW, In Progress, Review, Done; Size: S/M/L/XL).
- The user prioritizes and moves cards from NEXT to NOW. Only take tasks that are in **NOW**.
- Start: a branch per task linked to the issue (`gh issue develop <n> --name <n>-short-slug
  --base main`), assign the issue (`gh issue edit <n> --add-assignee @me`), move the card to
  **In Progress** (`gh project item-edit 6 --owner ewiggin --url <issue url> --field Status --value
  "In Progress"`). Work and commit on that branch (change, then its `CHANGES.md` note), never on main.
- Finish: push the branch, move the card to **Review** and tell the user; they open the PR (with
  `Closes #<n>`) and review it.
- Review feedback: apply the requested changes on the same branch and push.
- The user merges the PR; only then move the card to **Done** (the issue closes via `Closes #<n>`).
- Static build: `make static` / `make install-static` / `make dist` (musl target
  `x86_64-unknown-linux-musl`, needs `musl-gcc` for bundled SQLite; tests also pass with
  `cargo test --target x86_64-unknown-linux-musl`). Release profile has `strip = true`.
- macOS (Apple Silicon only for releases): `make dist` builds `DIST_TARGET` (musl on Linux,
  `aarch64-apple-darwin` on macOS). Makefile and install.sh must work with BSD tools: no
  `install -D`/`-t`, `sha256sum` falls back to `shasum -a 256`.
- CI (`.github/workflows/ci.yml`): tests, clippy `-D warnings`, `make lint-man`, shellcheck of
  install.sh, static build; macOS job (tests, clippy, `make dist`). Release (`release.yml`): on tag
  `vX.Y.Z` (must match Cargo.toml), builds `make dist` for x86_64 and aarch64 musl and
  aarch64-apple-darwin (`MACOSX_DEPLOYMENT_TARGET` 11.0) and publishes `sshh-<target>.tar.gz` + `.sha256`
  (no version in the name, so `releases/latest/download/...` works); the release body is the
  version's section of CHANGES.md (`scripts/release-notes.sh vX.Y.Z`; the build fails without it). License: MIT OR Apache-2.0.
- Versions: `make version` alone shows the current version, the latest tag and the next ones;
  `make version vX.Y.Z|patch|minor|major` (`scripts/bump.sh`, POSIX sh, GNU and BSD
  tools: no `sed -i`, use its `rewrite`) updates Cargo.toml, Cargo.lock, the man page `.TH` line and
  CHANGES.md (`## Unreleased` → `## X.Y.Z — date`; fails if it's missing or empty) and commits
  "bump to vX.Y.Z"; then tag + push.
- crates.io: the package is `sshh-tui` (`sshh` is taken) with `[[bin]] name = "sshh"`;
  `[package.metadata.binstall]` points to the release tarballs (keep it in sync with their names).
  `cargo publish` after pushing the release tag; `exclude` keeps CI/scripts out of the crate.
- Install: `make build && make install` (PREFIX defaults to `~/.local`; `install` never builds;
  plain `make` only lists the targets, documented with `## ` comments in the Makefile), or
  `install.sh` (POSIX sh, used as `curl … | sh`): downloads the release tarball for Linux
  x86_64/aarch64 or macOS arm64 (also under Rosetta) and checks its sha256; falls back to building from source (clone + make), and
  always builds when run from a clone or with `SSHH_FROM_SOURCE`/`SSHH_REF`. Uses sudo only for the
  copy. Test with `SSHH_RELEASES=http://127.0.0.1:<port>` (python http.server serving
  `latest/download/<asset>`), `SSHH_REPO=file://…`, `SSHH_PREFIX=<tmp>`; keep it sh/dash compatible.
- Manual tests without connecting: `SSHH_DB=/tmp/x.db SSHH_SSH=<script printing its args> sshh ...`.
  Set these per command; don't `export` them in the user's shell.
- `SSHH_DEBUG=1` prints to stderr how the destination is resolved and the command that runs.
- The TUI can be tested with tmux: `tmux new -d -s t -x 120 -y 30 sshh` + `send-keys` / `capture-pane -p`.
- `SSHH_INCLUDE_FILE` redirects the generated file; to test `include_state` use `HOME=<tmp>`
  (note: the real `ssh` reads the home from /etc/passwd, not `$HOME`; use `ssh -F <config>`).
- Language: English for UI messages, comments, docs and tests. Quote values with single quotes
  (`'web'`) in messages.
