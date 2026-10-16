# sshh

[![CI](https://github.com/ewiggin/sshh/actions/workflows/ci.yml/badge.svg)](https://github.com/ewiggin/sshh/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/ewiggin/sshh)](https://github.com/ewiggin/sshh/releases)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

SSH connection manager for the terminal, written in Rust.

- Use it **just like `ssh`**: `sshh user@host`, `sshh -p 2222 -J bastion web`, etc.
- When the connection is new, a **wizard** offers to save it with a name, description, tags and notes.
- Without arguments it opens a **TUI** (lazygit style) with fuzzy search, keyboard navigation (vim
  style) and mouse support, where you can keep **several ssh sessions open side by side, in
  columns**, next to the connection list.
- **It doesn't reimplement SSH**: it always runs the system `ssh`, so your `~/.ssh/config`,
  `ssh-agent`, `ProxyJump`, `ControlMaster`, `known_hosts`, FIDO keys, etc. keep working.
- **It doesn't store passwords** or any secrets: use your keys and your agent as usual.

> Status: in development. See [CHANGES.md](CHANGES.md) for what changed in each version.

## Contents

- [Installation](#installation)
- [Quick start](#quick-start)
- [Wrapper mode: `sshh` as `ssh`](#wrapper-mode-sshh-as-ssh)
- [New connection wizard](#new-connection-wizard)
- [TUI](#tui)
- [Connection form](#connection-form)
- [Persistent sessions (tmux)](#persistent-sessions-tmux)
- [Subcommands](#subcommands)
- [Importing `~/.ssh/config`](#importing-sshconfig)
- [Export and import (JSON)](#export-and-import-json)
- [History](#history)
- [Integration with ssh, scp, rsync, git…](#integration-with-ssh-scp-rsync-git)
- [Data and storage](#data-and-storage)
- [Environment variables](#environment-variables)
- [Development](#development)
- [License](#license)

## Installation

```sh
curl -fsSL https://raw.githubusercontent.com/ewiggin/sshh/main/install.sh | sh
```

On Linux x86_64 and aarch64 (a **static binary**) and on macOS with Apple Silicon the script
downloads the binary of the latest [release](https://github.com/ewiggin/sshh/releases), checks its
SHA-256 and installs it with the man page for your user, in `~/.local/bin/sshh` and `~/.local/share/man/man1/sshh.1` (no sudo needed),
so `man sshh` works. It only needs `curl` (or `wget`) and OpenSSH. On other systems, or if the
download fails (e.g. Intel Macs), it builds sshh from source, which needs Rust ≥ 1.88, git and make
(on macOS, `xcode-select --install`). It warns you if `~/.local/bin` is not in your `PATH` (on
macOS it isn't by default: add `export PATH="$HOME/.local/bin:$PATH"` to `~/.zshrc`). Run it again to
update.

| Variable / option | Effect |
|---|---|
| `SSHH_PREFIX=/usr/local` | install somewhere else (uses `sudo` only for the final copy if needed; as root the default is `/usr/local`) |
| `SSHH_VERSION=v0.1.0` | install that release instead of the latest |
| `SSHH_FROM_SOURCE=1` | always build from source |
| `SSHH_REF=main` | build that branch or tag from source |
| `sh -s -- --uninstall` | remove the binary and the man page (your connections are kept) |

You can also download the tarball for your system from the releases page
(`sshh-x86_64-unknown-linux-musl`, `sshh-aarch64-unknown-linux-musl`, `sshh-aarch64-apple-darwin`):
it contains the binary, the man page and the licenses.

```sh
curl -fsSL https://raw.githubusercontent.com/ewiggin/sshh/main/install.sh | SSHH_PREFIX=/usr/local sh
curl -fsSL https://raw.githubusercontent.com/ewiggin/sshh/main/install.sh | sh -s -- --uninstall
```

### From a clone

```sh
git clone https://github.com/ewiggin/sshh.git && cd sshh && make build && make install
```

`./install.sh` also works inside a clone (it installs that code). To update a clone, run
`git pull && make build && make install`.

Other options, from the cloned directory:

```sh
make                                                # list every target with a description
make build && sudo make install PREFIX=/usr/local   # system wide
make install DESTDIR="$pkgdir" PREFIX=/usr          # packaging
make uninstall                                      # same PREFIX/DESTDIR as when installing
make man                                            # preview the man page without installing it
```

`make install` doesn't compile, so `sudo make install` never runs cargo as root.

### Static binary (musl)

A fully static build runs on any x86_64 Linux, whatever its glibc version, with no dependencies
(SQLite is bundled):

```sh
make static && make install-static     # build and install it
make dist                              # dist/sshh-x86_64-unknown-linux-musl.tar.gz (+ .sha256)
```

The tarball contains `sshh`, `sshh.1` and this README, ready to attach to a release or copy to another
machine. Building it needs the musl Rust target and `musl-gcc`:

| System | Packages |
|---|---|
| rustup | `rustup target add x86_64-unknown-linux-musl` (+ `musl-gcc` from your distribution) |
| Arch | `rust-musl` and `musl` |
| Debian / Ubuntu | `musl-tools` (+ the rustup target) |

On macOS `make dist` builds the native binary instead (`dist/sshh-aarch64-apple-darwin.tar.gz`; it
only links system libraries).

Binary only, without the man page and without cloning (installs `sshh` to `~/.cargo/bin`; the
crate is called `sshh-tui` because `sshh` was taken on crates.io):

```sh
cargo install --locked sshh-tui     # or `cargo binstall sshh-tui` to download the release binary
cargo install --locked --git https://github.com/ewiggin/sshh.git     # latest unreleased code
```

Optionally, to always use it instead of `ssh`:

```sh
# ~/.bashrc / ~/.zshrc
alias ssh=sshh
```

Anything `sshh` doesn't understand is passed to `ssh` unchanged, so the alias is safe. If you ever
install `sshh` under the name `ssh` in your `PATH`, it detects itself and looks for the real `ssh`.

## Quick start

```sh
sshh                          # open the TUI
sshh root@10.0.0.5            # connect; if it's new, offer to save it
sshh web                      # connect to the saved connection with alias "web"
sshh web uptime               # remote command, as with ssh
sshh -L 8080:localhost:80 web # any ssh option works
sshh ls                       # list saved connections
sshh import-ssh-config        # import the Host blocks of your ~/.ssh/config
sshh history                  # latest connections
```

## Wrapper mode: `sshh` as `ssh`

`sshh [ssh options] destination [command]` accepts exactly the same arguments as `ssh` (including
`ssh://user@host:port`). Depending on the destination:

| Case | What it does |
|---|---|
| The destination is the **alias** of a saved connection | Adds `-o HostName=… -o User=… -o Port=…` (plus identity, ProxyJump and extra options) before the destination, **only for what you didn't set yourself**. Your flags (`-p`, `-l`, `-i`, `-J`, `-o`) always win. |
| The destination **matches** a saved connection (same `user@host:port`) | Runs `ssh` with your arguments untouched and records it in the history. |
| The destination **is not saved** | Opens the [wizard](#new-connection-wizard) (interactive terminals only). |
| No destination (`-V`, `-Q cipher`…) or arguments it doesn't understand | Passes them to `ssh` unchanged. |

To find out whether a destination is already saved, `sshh` asks `ssh -G` where it would really
connect, applying your `~/.ssh/config`. So `sshh db1` (a `Host` in your config) is recognised just like
`sshh admin@10.0.0.17 -p 2200`.

Calls that only query (`-G`, `-V`, `-O`, `-Q`) don't open the wizard and aren't recorded in the
history.

`sshh` never gets in the way of a connection: if something of its own fails (database, parser), it
warns and runs `ssh` with your original arguments.

To see what it decides and what exactly it runs:

```sh
SSHH_DEBUG=1 sshh web
# sshh[debug]: 'web' is the alias of a saved connection
# sshh[debug]: exec /usr/bin/ssh -o HostName=10.0.0.5 -o User=deploy -o Port=2222 web
```

To connect to a host named like a subcommand (`ls`, `add`, `rm`, `help`, `history`, `export`,
`import`, `import-ssh-config`, `ssh-config`), use `sshh -- ls`.

## New connection wizard

When connecting to a destination that isn't saved, a form shows up prefilled with what `ssh -G`
reports: host, user, port and ProxyJump. The suggested alias is the short host name
(`web.example.com` → `web`; IPs stay as they are).

| Key | Action |
|---|---|
| `Enter` | save and connect |
| `Esc` | connect without saving |
| `Ctrl-c` | cancel (doesn't connect) |

It only appears when stdin, stdout and stderr are all terminals, so it never gets in the way of
scripts. It can be disabled with `SSHH_NO_WIZARD=1`.

## TUI

`sshh` without arguments opens a lazygit-style layout:

```
╭ Search ──────────╮╭ 1 ● web ───────────────╮╭ 2 ● db1 ───────────────╮
╰──────────────────╯│deploy@web:~$ uptime    ││postgres@db1:~$ psql    │
╭ Connections ─────╮│ 12:01 up 3 days        ││psql (16.2)             │
│●     web  Web    ││deploy@web:~$ █         ││postgres=#              │
│●     db1  Data…  ││                        ││                        │
│   3M bastion     ││                        ││                        │
╰──────────────────╯│                        ││                        │
╭ Details ─────────╮│                        ││                        │
│Web production    ││                        ││                        │
╰──────────────────╯╰────────────────────────╯╰────────────────────────╯
   list = column 0      ◄── Alt-h ── Alt-l ──►     Alt-1 … Alt-9
```

- **Left**: search, the connection list (alias, name and tags) and the details of the selected one
  (target, options, tags, usage, notes). The list pane has three tabs, **Connections**, **Tags** and
  **Layouts** (switch with `[` / `]` or a click on the tab): the Tags tab lists every tag with its
  number of connections, the details pane previews the connections of the highlighted tag, and `Space`
  goes back to the connections filtered by that tag (`#tag` in the search box; `Esc` clears it). The
  Layouts tab lists the saved pane layouts (see below).
- **Right**: **terminal columns**, each showing an embedded session side by side: the ssh session of
  a connection or, with `s`, its sftp session (a connection can have both open at once). Connections with a live session are marked with a green `●` in the list (a grey `○` means
  the session ended); next to it (unless the session is alive), the time since the last connection, lazygit style (`5m`, `2d`,
  `3w`, `1M`, `1y`). Closing a column doesn't close its session. With no column open, the right side
  shows the actions and latest connections of the selected connection.
- **Sizes**: like lazygit, the left side takes a third of the width, and it gets narrower as columns
  are added (a quarter with two, a fifth with three…) down to 24% of the width (24 to 40 characters),
  which is what limits how many columns fit. On a narrow terminal (under 100
  columns) the layout goes vertical: search on top, the list and the details side by side, and the
  panes below, one under the other (as many as fit, at least 6 lines each). The panes also go one
  under the other when the columns don't fit side by side (at least 40 characters each). Then
  `Alt-j` / `Alt-k` go through every pane in order and `Alt-h` goes back to the list.

Each embedded session is the system `ssh` running inside a pseudo terminal, so everything works as in
a normal terminal (passwords, host key prompts, agent, full-screen programs like `vim`, `less` or
`htop`):

- It answers the queries programs send to their terminal (device attributes, cursor position,
  modes…), so programs like `fish` or `neovim` start without waiting for a timeout.
- Programs that use the mouse (`htop`, `vim` with `mouse=a`, `mc`…) receive clicks, drags and the
  wheel over their column. Hold `Shift` to select text with your terminal instead.
- Drag over a column to **select text and copy it** to the clipboard when you release the button:
  only that pane's text, without borders or other columns.
- It's resized with its column, keeps 5000 lines of history (mouse wheel, or `Alt-s` for history
  mode) and accepts pastes.

Quitting `sshh` closes the sessions (it asks first).

The UI only uses your terminal's 16-color palette and its default foreground and background, so it
follows your terminal theme (for example, Omarchy themes): borders and shortcuts use the theme's
blue, its primary accent.

The list can be sorted by recent use, by number of uses or by alias. If the output isn't a terminal
(`sshh | less`), it prints the list as text.

### Keyboard

Shortcuts follow the conventions of vim, less, lazygit and fzf.

Terminal columns and panes (these work from anywhere, also while typing in a session). A column
can hold several panes stacked vertically, like tmux or vim splits:

```
╭ 1.1 ● web ──────╮╭ 2 ● db1 ────────╮
│                 ││                 │
╰─────────────────╯│                 │
╭ 1.2 ● api ──────╮│                 │
│                 ││                 │
╰─────────────────╯╰─────────────────╯
```

| Key | Action |
|---|---|
| `Space` (in the list) | show the selected connection in the active pane (opens its session; focuses it if it's already in a pane; reconnects if it ended) |
| `Alt-v` | show the selected connection in a **new column** right of the active one |
| `Alt--` | show the selected connection in a **new pane below the last column** |
| `Alt-h` / `Alt-j` / `Alt-k` / `Alt-l` (or `Alt-arrows`) | focus the pane on the left / below / above / on the right; the list is the leftmost column (there, `Alt-j`/`Alt-k` select the next / previous connection with a session) |
| `Alt-1` … `Alt-9` | jump to column N |
| `Alt-Shift-j` | stack the active pane below the column on its right (or on its left, if it's the last one) |
| `Alt-Shift-k` | take the active pane out of its stack into its own column |
| `Alt-Shift-h` / `Alt-Shift-l` | move the active column left / right |
| `Alt-n` / `Alt-p` | show the next / previous session in the active pane (sessions visible in other panes are skipped) |
| `Alt-w` | close the pane; its neighbours take the space (the session keeps running) |
| `Alt-f` (or `Alt-z`) | full screen: the active pane takes the whole screen, inside `sshh` (again to go back) |
| `Alt-s` | history mode for the active pane (see below) |
| `Alt-a` | broadcast: what you type (or paste) in the active pane also goes to every other visible pane (again to stop) |

Columns share the width equally and need at least 40 characters each (up to 9 columns); panes in a
column share its height and need at least 6 lines each. Pane titles show their position (`2` for a
column with one pane, `1.1`, `1.2`… for stacked ones). A session is shown in at most one pane.

While **broadcast** is on, the panes that receive the input have a red border and `BROADCAST` in
their title, and the footer shows a `BROADCAST` badge. Only visible panes with a live session of the
same kind as the active one (ssh or sftp) take part: sessions without a pane, ended ones, panes in
history mode and, in full screen, every pane but the active one are left out. It is always off when
`sshh` starts and stops by itself when the last pane is closed.

List and general keys:

| Key | Action |
|---|---|
| `x` | close the selected connection's sessions, ssh and sftp (asks first) |
| `f` | full-screen ssh, as a plain terminal; you come back to `sshh` when it exits |
| `j` / `k`, `↓` / `↑` | move |
| `gg` / `G` | go to top / bottom |
| `Ctrl-d` / `Ctrl-u` | half page down / up |
| `Ctrl-f` / `Ctrl-b`, `PgDn` / `PgUp` | page down / up |
| `Tab` / `Shift-Tab` | next / previous pane: list → details → active column (if any; inside it `Tab` goes to ssh, leave with `Alt-h`) |
| `/` | search |
| `[` / `]` | previous / next tab: Connections / Tags / Layouts (in Tags, `Space` or `Enter` filters by the tag, `Esc` goes back) |
| `Alt-v` (on a tag, in Tags) | open every connection of the tag in panes, replacing the current ones (their sessions keep running); as many columns as fit, the rest stacked. Then `Alt-a` types in all of them |
| `a` | add a connection |
| `Enter`, `e` | edit |
| `t` | edit tags |
| `dd` | delete (asks for confirmation with `y`) |
| `yy` | copy the equivalent `ssh` command to the clipboard |
| `s` | open `sftp` with the connection in a new column, next to its ssh session |
| `c` | install your public key on the server (`ssh-copy-id`) |
| `o` | change sort order: recent → most used → alphabetical |
| `R` | reload the list |
| `?` | help |
| `Esc` | go back one level / clear the search (never quits) |
| `q`, `Ctrl-c` | quit (asks first if there are open sessions) |

**Layouts** save which connection is in which pane, to get the same panes back later. In the Layouts
tab:

| Key | Action |
|---|---|
| `a` | save the current panes as a layout: type a name and press `Enter` (an existing name replaces that layout) |
| `Space`, `Enter` | open the layout: its panes replace the current ones (their sessions keep running), opening the sessions that aren't alive |
| `r` | rename the layout: edit the name and press `Enter` (it can't take the name of another layout) |
| `dd` | delete the layout (asks for confirmation with `y`) |

The details pane previews the panes of the highlighted layout. A layout keeps ssh and sftp panes; the
panes of a deleted connection disappear from it. If the window is too small for the saved shape the
panes are rearranged in a grid, and those that don't fit are not opened. Layouts are stored in the
database but are not part of `sshh export`.

While a **terminal column has focus, every key goes to its ssh session** (including `Esc`, `Ctrl-c`,
`q`, `Tab` and `Ctrl-w`), except the `Alt` shortcuts above. Note that `Alt-f` (readline: forward a
word) and `Alt-←`/`Alt-→` (word movement in zsh/fish) are taken by `sshh`; use `Ctrl-←`/`Ctrl-→`
in the shell instead. Other readline keys (`Alt-.`, `Alt-b`, `Alt-d`…) still reach the shell. When a session has ended,
`Enter` or `Space` reconnects and `Esc` goes back to the list.

#### macOS: Option as Alt

On macOS the `Alt` shortcuts are the **Option (⌥)** key, but most terminals use Option to type
special characters by default (`⌥-h` types `˙`), so `sshh` (and tmux, vim…) never see `Alt-h`. Turn
it into Alt in your terminal; using only the left one keeps the right Option for characters like
`@`, `#` or `€`:

| Terminal | Setting |
|---|---|
| Terminal.app | Settings → Profiles → Keyboard → "Use Option as Meta key" |
| iTerm2 | Settings → Profiles → Keys → Left Option key: **Esc+** |
| Ghostty | `macos-option-as-alt = left` |
| kitty | `macos_option_as_alt left` |
| Alacritty | `[window] option_as_alt = "OnlyLeft"` |
| WezTerm | `send_composed_key_when_left_alt_is_pressed = false` |

In iTerm2 the native selection over a program that uses the mouse is Option-drag instead of `Shift`.

### History mode

`Alt-s` freezes the active column's history and lets you move through it with the keyboard, like
tmux's copy mode. The current line is highlighted and the column title shows the position.

| Key | Action |
|---|---|
| `j` / `k`, `↓` / `↑` | move a line |
| `Ctrl-d` / `Ctrl-u`, `Ctrl-f` / `Ctrl-b` | half page / page |
| `gg` / `G` | oldest / newest line |
| `/text` + `Enter` | search backwards (case insensitive) |
| `n` / `N` | previous / next match |
| `v` | start / cancel a line selection |
| `y`, `Enter` | copy the selection (or the current line) to the clipboard and leave |
| `q`, `Esc` | leave (`Esc` first cancels the selection) |

### Search

`/` starts a **fuzzy** search over alias, name, user, host, description and tags. The list is
filtered while you type and matching letters are highlighted.

- `#tag` filters by tag (prefix match): `#prod`, `#prod web`, `#prod #db`.
- `↑` / `↓`, `Ctrl-j` / `Ctrl-k` or `Ctrl-n` / `Ctrl-p` move without leaving the search.
- `Ctrl-w` deletes a word; `Ctrl-u` deletes everything.
- `Enter` or `Esc` go back to the list keeping the filter (then `Space` connects); a second `Esc`
  clears it.

### Mouse

| Action | Effect |
|---|---|
| Click on a tab title | switch to that tab |
| Click on a row | select (a tag in the Tags tab; double click filters by it; in the Layouts tab it opens the layout) |
| Double click | open the session (like `Space`) |
| Click on a column | focus it |
| Drag over a column | select its text; releasing copies it to the clipboard (highlighted until the next click or key) |
| Wheel | move · scroll the details · scroll back the history of the column under the pointer |
| Click, drag, wheel on a program that uses the mouse | sent to that program (`Shift` to select text natively) |
| Click on the search box | search |
| Click on the details | focus them |
| Click on a form field | edit that field |

## Connection form

The same form is used to add (`a`), edit (`e`, `t`) and in the wizard.

| Field | Notes |
|---|---|
| Alias | Required and unique. It's what you use in `sshh <alias>`. No spaces or `* ? ! , @ " #`. |
| Host | Required. Hostname or IP. |
| User, Port | Optional. Empty = whatever ssh decides (local user, 22). |
| Identity | Path to the key (`IdentityFile`). |
| ProxyJump | `bastion` or `user@host:port`. |
| tmux | Remote tmux session to attach to, see [persistent sessions](#persistent-sessions-tmux). Letters, digits, `-` and `_`. |
| Name, Description | Free text to identify it. |
| Tags | Separated by commas or spaces; the leading `#` is optional. |
| Options | Extra ssh_config options, one per line: `ForwardAgent=yes`. |
| Notes | Free multiline text. |

| Key | Action |
|---|---|
| `Tab` / `Shift-Tab`, `↓` / `↑` | next / previous field (in multiline fields the arrows move between lines) |
| `Enter` | save (inserts a line break in Options and Notes) |
| `Ctrl-s` | save from any field |
| `Ctrl-e` | edit Options or Notes in `$VISUAL` / `$EDITOR` (or `vi`) |
| `Ctrl-w` / `Ctrl-u` | delete word / up to the start of the line |
| `Home` / `Ctrl-a`, `End` | start / end of line |
| `Esc` | cancel (in the wizard: connect without saving) |

If a value is invalid (wrong port, duplicate alias…), focus jumps to that field and the error is shown
in red.

## Persistent sessions (tmux)

Sessions live inside `sshh`: closing it (or losing the network) ends them. To keep the shell alive,
give the connection a **tmux session** name (form field `tmux`, or `sshh add --tmux <name>`). Every
interactive connection then runs

```sh
ssh -t … web 'tmux new-session -A -s <name> || exec $SHELL -l'
```

which attaches to that remote tmux session, creating it the first time. Close `sshh`, open it again
(or open a [layout](#keyboard)) and you are back where you left off. tmux keeps one server per remote
user, so sessions only clash when several people log in as the same user (`root`, `deploy`…): use a
name of your own, like your name. Detach with tmux's `Ctrl-b d` to leave it running.

- It applies to the embedded sessions, `f` (full screen), `yy` and `sshh <alias>` in a terminal, but
  not when there is a remote command (`sshh web uptime`), with `-N`, `-T`, `-W`, `-s`, `-f`, a
  `RemoteCommand` option or no terminal (scripts, pipes), nor to sftp, scp or the generated ssh_config
  file.
- If the server has no tmux, you get a plain login shell.
- Scrollback is tmux's (`Ctrl-b [`), not `sshh`'s history mode, since tmux redraws the screen.

## Subcommands

Besides the TUI, there are subcommands for scripts or quick use:

```sh
sshh ls                                      # list as text
sshh add <alias> <destination> [options]     # save a connection
sshh rm <alias>                              # delete a connection
sshh history [alias] [-n N]                  # latest connections
sshh export [-o file]                        # export to JSON
sshh import <file|->                         # import from JSON
sshh import-ssh-config [file]                # import the Host blocks of ~/.ssh/config
sshh ssh-config [status|sync|print|disable]  # generated ssh_config file (see below)
sshh --help                                  # help (also `sshh <subcommand> --help`)
```

`sshh add` options:

| Option | Description |
|---|---|
| `-p, --port <PORT>` | port |
| `-i, --identity <PATH>` | key (`IdentityFile`) |
| `-J, --jump <DESTINATION>` | ProxyJump |
| `--tmux <NAME>` | remote tmux session to attach to ([persistent sessions](#persistent-sessions-tmux)) |
| `-o, --option <Key=Value>` | extra ssh_config option (repeatable) |
| `-n, --name <TEXT>` | name |
| `-d, --description <TEXT>` | description |
| `--notes <TEXT>` | notes |
| `-t, --tag <TAGS>` | comma separated tags (repeatable) |

```sh
sshh add web deploy@10.0.0.5 -p 2222 -i ~/.ssh/deploy -n "Web production" -t prod,web \
  -o ForwardAgent=yes --notes "Restart: sudo systemctl restart nginx"
```

## Importing `~/.ssh/config`

```sh
sshh import-ssh-config --dry-run          # see what would be imported
sshh import-ssh-config -t ssh-config      # import, adding the "ssh-config" tag
sshh import-ssh-config other/config       # another file
```

- Every `Host` block with concrete names is imported. `Host web1 web2` creates two connections.
- `HostName`, `User`, `Port`, `IdentityFile` and `ProxyJump` go to their fields; any other option
  (`LocalForward`, `ForwardAgent`, additional `IdentityFile`s…) is stored as an extra option.
- Without `HostName`, the host is the alias itself (as ssh does).
- Comments right above a `Host` become its notes.
- If an alias appears in several blocks, they are merged and the first value of each option wins, as
  in ssh.
- `Include`s are followed (with paths relative to `~/.ssh` and wildcards like `config.d/*`).
- **Not imported**: `Host` patterns with wildcards or negations (`Host *`, `Host *.lan`, `!foo`),
  `Match` blocks and global options. ssh still applies them when connecting. The command lists them at
  the end so you know which ones they are.

Options shared by `import` and `import-ssh-config`:

| Option | Description |
|---|---|
| `-n, --dry-run` | show what would happen without saving anything |
| `--on-conflict skip` | (default) if the alias already exists with different data, don't import it |
| `--on-conflict overwrite` | replace the existing one |
| `--on-conflict rename` | import it under another alias (`web-2`, `web-3`…) |

Connections that already exist with the same data are counted as "unchanged", so importing twice
never duplicates anything.

```text
$ sshh import-ssh-config
3 new
  + db1
  + web1
  + web2
```

## Export and import (JSON)

```sh
sshh export -o connections.json           # created with 0600 permissions (notes may be sensitive)
sshh export > connections.json
sshh import connections.json
sshh import --on-conflict rename connections.json
cat connections.json | sshh import -
```

The export includes all the data of every connection, but not the history. Format:

```json
{
  "version": 1,
  "exported_at": "2026-10-07T13:27:42+02:00",
  "hosts": [
    {
      "alias": "web",
      "hostname": "10.0.0.5",
      "user": "deploy",
      "port": 2222,
      "identity_file": "~/.ssh/deploy",
      "proxy_jump": "bastion",
      "extra_options": [{ "key": "ForwardAgent", "value": "yes" }],
      "name": "Web production",
      "description": "nginx frontend",
      "notes": "Restart: sudo systemctl restart nginx",
      "tags": ["prod", "web"]
    }
  ]
}
```

Only `alias` and `hostname` are required. `sshh import` also accepts a plain list
`[{...}, {...}]`, which makes it easy to generate the file from other tools. Invalid entries are
skipped with their reason and the rest is imported.

## History

Every connection to a saved host that goes through `sshh` is recorded with its date and arguments.

```text
$ sshh history
DATE              ALIAS  COMMAND
2026-10-07 13:27  web    sshh web
2026-10-07 13:27  db1    sshh -v db1
2026-10-07 13:27  web    sshh web uptime
```

- `sshh history web` shows only one connection's entries; `-n 50` changes the limit (20 by default).
- In the TUI, the details pane shows the latest 5 of the selected connection, and the list can be
  sorted by recent use or by number of uses (`o`).
- Calls that only query (`-G`, `-V`, `-O`, `-Q`) and connections to unsaved destinations are not
  recorded.

## Integration with ssh, scp, rsync, git…

`sshh` can maintain a `~/.ssh/config.d/sshh.conf` file with one `Host` block per saved connection.
Once it's included from your `~/.ssh/config`, **any tool** that uses ssh knows your aliases even
without going through `sshh`: `ssh web`, `scp file web:`, `rsync -a dir/ web:`, `git clone web:repo`,
VS Code Remote-SSH, Ansible…

**`sshh` never modifies your `~/.ssh/config`.** It only writes its own file; you add the line that
includes it.

### Enabling it

1. Add this line **at the top** of `~/.ssh/config`, before any `Host` or `Match`:

   ```
   Include config.d/sshh.conf
   ```

   It must go at the top: in ssh_config, an `Include` written after a `Host` block is part of that
   block and would only apply to that host. The relative path is resolved from `~/.ssh`. If the file
   doesn't exist yet, ssh just ignores it, so you can add the line before step 2.

2. Generate the file:

   ```sh
   sshh ssh-config sync
   ```

3. Check that everything is fine:

   ```sh
   sshh ssh-config            # status: enabled and correctly included
   ssh -G web | head          # ssh now resolves the alias
   ```

From then on the file is regenerated automatically every time you save, edit, delete or import
connections (from the TUI, the wizard or the CLI).

| Command | What it does |
|---|---|
| `sshh ssh-config` / `status` | tells whether it's enabled and whether `~/.ssh/config` includes it (and whether the line is misplaced) |
| `sshh ssh-config sync` | generates the file now and enables it |
| `sshh ssh-config print` | shows what would be generated, without writing anything |
| `sshh ssh-config disable` | deletes the file and stops updating it (the `Include` line can stay) |

Example generated file (`0600` permissions, written atomically). Notes are **not** included:

```sshconfig
# Generated by sshh: do not edit, it is overwritten on every change.

# Web production — nginx frontend
Host web
    HostName 10.0.0.5
    User deploy
    Port 2222
    IdentityFile ~/.ssh/deploy
    LocalForward 8080 localhost:80
```

**If an alias is both in your `~/.ssh/config` and in `sshh`** (for example after
`sshh import-ssh-config`), with the `Include` at the top `sshh`'s value wins for every option it
defines, because ssh keeps the first value it finds. Options that are only in your config still apply.
If you want `sshh` to be the source of truth, you can delete those blocks from your config.
`sshh import-ssh-config` never imports the generated file.

## Data and storage

- SQLite database at `~/.local/share/sshh/sshh.db`, or `$XDG_DATA_HOME/sshh/sshh.db` if set (directory
  with `0700` permissions). macOS uses the same path.
- Stored: alias, host, user, port, identity, ProxyJump, extra options, name, description, notes, tags
  and connection history (date and arguments).
- **No passwords or keys are stored.**
- The schema migrates itself on startup.
- Optionally, `~/.ssh/config.d/sshh.conf` (see [Integration](#integration-with-ssh-scp-rsync-git)),
  generated from the database: SQLite remains the source of truth.

## Environment variables

| Variable | Effect |
|---|---|
| `SSHH_DEBUG=1` | prints to stderr how the destination is resolved and the exact command that runs |
| `SSHH_NO_WIZARD=1` | don't open the wizard for new destinations |
| `SSHH_DB=<path>` | use another database (useful for testing) |
| `SSHH_SSH=<path>` | use another binary instead of `ssh` |
| `SSHH_INCLUDE_FILE=<path>` | another path for the generated ssh_config file (useful for testing) |
| `VISUAL` / `EDITOR` | editor for `Ctrl-e` in the form |

The clipboard (`yy`) uses `pbcopy` on macOS, `wl-copy` (Wayland) or `xclip` (X11) when available and otherwise the
OSC 52 escape sequence, which most terminals support and which also works over ssh.

## Development

```sh
cargo test
cargo clippy --all-targets
make lint-man        # check the man page with groff
```

To test without connecting anywhere, point `SSHH_SSH` to something that prints its arguments and use
a separate database. Set the variables per command, so they don't stay in your shell:

```sh
SSHH_DB=/tmp/test.db cargo run -- add web deploy@10.0.0.5 -p 2222
SSHH_DB=/tmp/test.db SSHH_SSH=echo SSHH_DEBUG=1 cargo run -- web uptime
```

The TUI can be tested automatically with tmux:

```sh
tmux new -d -s t -x 120 -y 30 target/debug/sshh
tmux send-keys -t t / w e b
tmux capture-pane -p -t t
```

### Layout

| Module | Responsibility |
|---|---|
| `src/main.rs` | entry point: TUI, subcommand or wrapper mode |
| `src/ssh_args.rs` | OpenSSH command line parser |
| `src/connect.rs` | wrapper mode: destination resolution (`ssh -G`), wizard, `exec` of ssh |
| `src/db.rs` | SQLite, migrations (`PRAGMA user_version`), imports and history |
| `src/model.rs` | data model and validation |
| `src/ssh_config.rs` | reads ssh_config files (with `Include`) to import them |
| `src/include.rs` | generated ssh_config file (`~/.ssh/config.d/sshh.conf`) |
| `src/cli.rs` | subcommands (`ls`, `add`, `rm`, `history`, `export`, `import`, `import-ssh-config`, `ssh-config`) |
| `src/tui/app.rs` | TUI state and event handling (no terminal; testable) |
| `src/tui/form.rs` | add/edit form and wizard |
| `src/tui/ui.rs` | rendering |
| `src/tui/session.rs` | embedded sessions: ssh in a pseudo terminal + VT100 emulator, key encoding |
| `src/tui/term.rs` | terminal, `$EDITOR`, external programs and clipboard |

### Releasing

1. Make sure the `## Unreleased` section of [CHANGES.md](CHANGES.md) lists the changes.
2. Bump the version and commit it:

   ```sh
   make version             # shows the current version, the latest tag and the next ones
   make version v0.2.0      # or: make version patch | minor | major
   ```

   It updates `Cargo.toml`, `Cargo.lock`, the man page (version and date) and `CHANGES.md`
   (`## Unreleased` becomes `## 0.2.0 — <date>`) and commits them as `bump to v0.2.0`. It refuses to
   run with uncommitted changes, a version that isn't greater than the current one, an existing tag,
   or a missing or empty `## Unreleased` section. It works with GNU and BSD tools (Linux and macOS).
3. Tag and push (the command prints it): `git tag v0.2.0 && git push origin main v0.2.0`.

The release workflow checks that the tag matches `Cargo.toml`, runs the tests and publishes static
binaries for x86_64 and aarch64 Linux and a binary for macOS on Apple Silicon, with their SHA-256
checksums.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
