# sshh

SSH connection manager for the terminal, written in Rust.

- Use it **just like `ssh`**: `sshh user@host`, `sshh -p 2222 -J bastion web`, etc.
- When the connection is new, a **wizard** offers to save it with a name, description, tags and notes.
- Without arguments it opens a **TUI** with fuzzy search, keyboard navigation (vim style) and mouse
  support.
- **It doesn't reimplement SSH**: it always runs the system `ssh`, so your `~/.ssh/config`,
  `ssh-agent`, `ProxyJump`, `ControlMaster`, `known_hosts`, FIDO keys, etc. keep working.
- **It doesn't store passwords** or any secrets: use your keys and your agent as usual.

> Status: in development. See the [Roadmap](#roadmap).

## Contents

- [Installation](#installation)
- [Quick start](#quick-start)
- [Wrapper mode: `sshh` as `ssh`](#wrapper-mode-sshh-as-ssh)
- [New connection wizard](#new-connection-wizard)
- [TUI](#tui)
- [Connection form](#connection-form)
- [Subcommands](#subcommands)
- [Importing `~/.ssh/config`](#importing-sshconfig)
- [Export and import (JSON)](#export-and-import-json)
- [History](#history)
- [Integration with ssh, scp, rsync, git…](#integration-with-ssh-scp-rsync-git)
- [Data and storage](#data-and-storage)
- [Environment variables](#environment-variables)
- [Development](#development)
- [Roadmap](#roadmap)

## Installation

Requirements: Rust ≥ 1.88 (2024 edition) and OpenSSH. SQLite is bundled into the binary.

```sh
cargo build --release
install -Dm755 target/release/sshh ~/.local/bin/sshh
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

`sshh` without arguments opens the list of connections, a details pane (with the notes and the latest
connections of the selected one) and a shortcuts bar. The list can be sorted by recent use, by number
of uses or by alias. If the output isn't a terminal (`sshh | less`), it prints the list as text.

### Keyboard

Shortcuts follow the conventions of vim, less, lazygit and fzf.

| Key | Action |
|---|---|
| `Enter` | connect |
| `j` / `k`, `↓` / `↑` | move |
| `gg` / `G` | go to top / bottom |
| `Ctrl-d` / `Ctrl-u` | half page down / up |
| `Ctrl-f` / `Ctrl-b`, `PgDn` / `PgUp` | page down / up |
| `Tab` | focus list ↔ details (to scroll long notes) |
| `/` | search |
| `a` | add a connection |
| `e` | edit |
| `t` | edit tags |
| `dd` | delete (asks for confirmation with `y`) |
| `yy` | copy the equivalent `ssh` command to the clipboard |
| `s` | open `sftp` with the connection |
| `c` | install your public key on the server (`ssh-copy-id`) |
| `o` | change sort order: recent → most used → alphabetical |
| `R` | reload the list |
| `?` | help |
| `Esc` | go back one level / clear the search (never quits) |
| `q`, `Ctrl-c` | quit |

### Search

`/` starts a **fuzzy** search over alias, name, user, host, description and tags. Matching letters
are highlighted.

- `#tag` filters by tag (prefix match): `#prod`, `#prod web`, `#prod #db`.
- `Enter` connects to the selected result (like fzf).
- `↑` / `↓`, `Ctrl-j` / `Ctrl-k` or `Ctrl-n` / `Ctrl-p` move without leaving the search.
- `Ctrl-w` deletes a word; `Ctrl-u` deletes everything.
- `Esc` goes back to the list keeping the filter; a second `Esc` clears it.

### Mouse

| Action | Effect |
|---|---|
| Click on a row | select |
| Double click | connect |
| Wheel | move (or scroll when the pointer is over the details) |
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

- SQLite database at `~/.local/share/sshh/sshh.db` (directory with `0700` permissions).
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

The clipboard (`yy`) uses `wl-copy` (Wayland) or `xclip` (X11) when available and otherwise the
OSC 52 escape sequence, which most terminals support and which also works over ssh.

## Development

```sh
cargo test
cargo clippy --all-targets
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
| `src/tui/term.rs` | terminal, `$EDITOR` and clipboard |

## Roadmap

- [x] **Phase 1**: database, migrations, ssh argument parser and `exec`.
- [x] **Phase 2**: TUI with list, details, fuzzy search, tag filter and mouse.
- [x] **Phase 3**: new connection wizard, add/edit/delete in the TUI, tags and copy command.
- [x] **Phase 4**: history, JSON export/import, `~/.ssh/config` import.
- [x] **Phase 5**: generated ssh_config file for `Include` and quick actions (sftp, ssh-copy-id).

Pending ideas: "don't ask again" for a destination in the wizard, `sshh add` without arguments
opening the form, configurable shortcuts.
