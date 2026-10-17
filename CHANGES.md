# Changes

All notable changes to sshh, newest first. Versions match the git tags `vX.Y.Z`.

## Unreleased

- **Panes on startup.** `*` in the Layouts tab makes a layout open when `sshh` starts (marked `★`);
  without one, `restore_panes = true` in `~/.config/sshh/config.toml` brings back the panes you had
  when you quit. They open without connecting: `Enter` connects each pane
  ([#21](https://github.com/ewiggin/sshh/issues/21)).

## 0.9.3 — 2026-10-17

- `Ctrl-n` in the wizard connects without saving and never asks again for that destination (for
  temporary IPs, CI machines…). `sshh ignore` lists them, `sshh ignore add <dest>` adds one and
  `sshh ignore rm <dest>` asks again; saving the destination also takes it off the list
  ([#3](https://github.com/ewiggin/sshh/issues/3)).

## 0.9.2 — 2026-10-17

- Releases are published to crates.io (`cargo install sshh-tui`) automatically, right after the
  GitHub release ([#9](https://github.com/ewiggin/sshh/issues/9)).

## 0.9.1 — 2026-10-17

- The help (`?`) scrolls (`j`/`k`, `Ctrl-d`/`Ctrl-u`, `gg`/`G`, wheel) and `/` searches it: matches are
  highlighted and `n`/`N` jump between them ([#12](https://github.com/ewiggin/sshh/issues/12)).

## 0.9.0 — 2026-10-16

- **Themes.** Besides the default `terminal` theme (the terminal's own colors), sshh ships
  `gruvbox`, `everforest`, `kanagawa`, `monokai` and `material` themes, which also recolor what runs
  in the panes, so sshh looks right on terminals with a poor palette (like the macOS Terminal).
  Choose one with `T` in the TUI, `theme = "…"` in `~/.config/sshh/config.toml` or `SSHH_THEME`; add
  your own in `~/.config/sshh/themes/`; list them with `sshh themes`
  ([#7](https://github.com/ewiggin/sshh/issues/7)).
- `sshh add` without a destination opens the add form, prefilled with the alias and options given
  ([#2](https://github.com/ewiggin/sshh/issues/2)).
- The GitHub release notes are the version's section of this file.
- The README explains how to run the macOS binary when the tarball was downloaded with a browser
  (`xattr -d com.apple.quarantine sshh`).

## 0.8.1 — 2026-10-16

- Fix the shellcheck warning (SC2016) in `scripts/bump.sh` that made CI fail.

## 0.8.0 — 2026-10-16

- **macOS support.** Releases include a binary for Apple Silicon (`sshh-aarch64-apple-darwin`) and
  `install.sh` installs it (also from a shell under Rosetta). Intel Macs build from source.
- The database is at `~/.local/share/sshh/sshh.db` on every platform, also on macOS (not
  `~/Library/Application Support`); `$XDG_DATA_HOME` is respected.
- The clipboard uses `pbcopy` on macOS.
- The README and man page explain how to make Option act as Alt on macOS terminals, which the pane
  shortcuts need.
- The Makefile works with BSD tools; `make dist` builds the native binary on macOS.
- CI runs the tests, clippy and `make dist` on macOS too.
- `make version` also renames `## Unreleased` in `CHANGES.md` to the new version (and fails if that
  section is missing or empty), and works on macOS. Without arguments it shows the current
  version, the latest tag and the next ones.

## 0.7.0 — 2026-10-15

- **Responsive layout**, like lazygit: the list takes a third of the width and shrinks as columns
  are added, down to its old size. Under 100 columns the layout goes vertical (list and details on
  top, panes below), and panes also stack when the columns don't fit side by side; `Alt-j/k` then
  walk every pane.

## 0.6.0 — 2026-10-15

- **Select and copy a pane's text by dragging the mouse** over a pane whose program doesn't use the
  mouse; releasing copies it to the clipboard, without borders or other columns. Shift keeps the
  terminal's native selection.

## 0.5.0 — 2026-10-15

- **Persistent sessions:** a connection can name a remote tmux session (form field, `add --tmux`,
  JSON). Interactive connections then run `tmux new-session -A -s <name>`, so the shell survives
  sshh and network drops. Not used with a remote command, `-N/-T/-W/-s/-f`, `RemoteCommand`, without
  a tty, for sftp or in the generated ssh_config file.
- The list shows the time since the last connection (`5m`, `2d`, `3w`…), hidden while its session
  is alive.
- Rename layouts with `r` in the Layouts tab.

## 0.4.0 — 2026-10-13

- **Broadcast** (`Alt-a`): what is typed or pasted in the active pane also goes to the other visible
  panes with a live session of the same kind (red border and a footer badge while it's on).
- **Open a tag** (`Alt-v` in the Tags tab): replaces the panes with a grid of the tag's connections,
  reusing live sessions.
- **Layouts tab:** save the current panes with a name (`a`), restore them (Space) and delete them
  (`dd`). Panes of deleted connections are dropped.
- Plain `make` lists every target with its description.

## 0.3.1 — 2026-10-08

- Published on crates.io as `sshh-tui` (the binary is still `sshh`), with `cargo binstall`
  metadata pointing to the release tarballs.
- The README recommends `cargo install --locked`.

## 0.3.0 — 2026-10-07

- **Stacked panes:** columns can hold several panes one under the other.
  - `Alt-h/j/k/l` (and `Alt`-arrows) move the focus in four directions.
  - `Alt-J` stacks the active pane below the column on its right (or left); `Alt-K` takes it out
    into its own column; `Alt-H/L` still move columns.
  - `Alt--` opens the selected connection in a new pane below the last column.
  - Changing the session shown in a pane moves to `Alt-n` / `Alt-p`.

## 0.2.1 — 2026-10-07

Version bump only; it was never tagged, and its changes shipped in 0.3.0.

- `s` opens **sftp as an embedded session** in a new column instead of suspending the TUI. A
  connection can have ssh and sftp open side by side; `x` closes both.

## 0.2.0 — 2026-10-07

- **Embedded sessions answer terminal queries** (DA1/DA2, DSR/cursor position, text area size,
  XTVERSION, DECRQM), so programs like neovim or fish don't wait for a timeout.
- **Mouse forwarding** to programs that enable mouse reporting (X10, normal, button and any-motion
  modes; default, UTF-8 and SGR encodings).
- **History mode** (`Alt-s`), like tmux's copy mode: move through the history with the keyboard,
  search with `/` and `n/N`, select lines with `v` and copy them with `y`.
- `make version` bumps the version and commits it.
- `install.sh` fix (shellcheck SC2015); GitHub Actions moved to Node 24.

## 0.1.0 — 2026-10-07

First release.

- **ssh wrapper:** `sshh` accepts the same arguments as ssh and always runs the system ssh, so
  ssh_config, ssh-agent, ProxyJump, etc. keep working. No passwords are stored.
- Connections in SQLite with name, description, notes, tags and history.
- **Wizard** to save new destinations, prefilled with `ssh -G`.
- **TUI** (ratatui) with fuzzy search, `#tag` filter, vim-style shortcuts, mouse, add/edit/delete,
  copy command, sftp and ssh-copy-id.
- JSON import/export and `~/.ssh/config` import (follows `Include`).
- Generated `~/.ssh/config.d/sshh.conf`, so ssh, scp, rsync or git can use the aliases;
  `~/.ssh/config` is never modified.
- **Embedded ssh sessions** next to the list, lazygit style: several at once, in a pseudo terminal
  with a VT100 emulator, with scrollback and bracketed paste.
- **Terminal columns** side by side: `Alt-v` new column, `Alt-1..9` jump, `Alt-w` close,
  `Alt-z`/`Alt-f` full screen, `Alt-H/L` move columns.
- Space connects and Enter edits, like lazygit.
- **Tags** shown in the list and a Tags tab (`[` / `]`).
- The sshh logo when no column is open.
- The UI uses only the 16 ANSI colors, so it follows the terminal theme.
- The cursor is shown again when leaving the TUI.
- Man page, Makefile and `install.sh` (downloads the release binary for Linux x86_64/aarch64,
  checks its SHA-256, falls back to building from source).
- Static musl build (`make static`, `make dist`).
- MIT OR Apache-2.0 license; CI and release workflows.
