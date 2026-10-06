# sshh

Gestor de conexiones SSH en Rust: TUI (ratatui + crossterm, teclado y ratón) y wrapper de `ssh`.
Se usa como `sshh [args de ssh]` (opcionalmente `alias ssh=sshh`).

## Principios
- **Nunca reimplementar SSH**: siempre se hace `exec` del `ssh` del sistema (`connect::exec_ssh`).
  sshh no debe impedir una conexión: si falla el parser o la DB, avisa y pasa los args tal cual.
- **No se guardan secretos** (contraseñas): claves y ssh-agent.
- Fuente de verdad: SQLite en `~/.local/share/sshh/sshh.db` (`$SSHH_DB` para override).
  Import/export en JSON (formato = `model::HostData`).
- Compatibilidad con el ecosistema: `include.rs` genera `~/.ssh/config.d/sshh.conf` (activado si el
  fichero existe; `include::refresh` tras cada cambio en la DB). **Nunca se modifica `~/.ssh/config`**:
  el usuario añade a mano `Include config.d/sshh.conf` al principio (después de un `Host` sería
  condicional a ese bloque).
- Valores de opciones: usar `model::config_value` (solo entrecomilla opciones de un único valor; las
  de varios argumentos como `LocalForward 8080 host:80` no deben llevar comillas).

## Módulos
- `ssh_args`: parser de la CLI de OpenSSH (flags con/sin argumento, destino, `ssh://`, `--`).
- `connect`: modo wrapper. Si el destino es un alias guardado, inyecta `-o Key=Value` antes del
  destino solo para lo que el usuario no fijó. Si no, resuelve el destino efectivo con `ssh -G`
  (aplica ssh_config) y lo busca por `user@hostname:port`; si coincide, pasa args intactos.
  Si no está guardado y la sesión es interactiva, abre el wizard (`SSHH_NO_WIZARD=1` lo desactiva).
  `-G/-V/-O/-Q` (`info_only`) ni abren wizard ni cuentan en el historial.
- `db`: SQLite + migraciones por `PRAGMA user_version` (añadir al final de `MIGRATIONS`, nunca editar).
- `tui`: `sshh` sin args y wizard. `app.rs` = estado + eventos (testeable sin terminal); los efectos
  (DB, portapapeles, $EDITOR) se piden vía `App::request` y los ejecuta `mod.rs`. `form.rs` = formulario
  de alta/edición/wizard. `ui.rs` = render. `term.rs` = terminal, $EDITOR y portapapeles
  (wl-copy/xclip, fallback OSC 52). Al conectar sale del TUI y llama a `connect::run`.
- Atajos: estilo vim/lazygit (`j/k`, `gg/G`, `Ctrl-d/u/f/b`, `/`, `a`, `e`, `t`, `dd`, `yy`, `R`, `Tab`, `?`).
  `Esc` siempre retrocede un nivel; solo `q`/`Ctrl-c` salen.
- `cli`: subcomandos propios (`ls`, `add`, `rm`, `history`, `export`, `import`, `import-ssh-config`);
  sus nombres están en `model::RESERVED_ALIASES` (añadir ahí cualquier subcomando nuevo).
- `ssh_config`: parser de ssh_config para importar `Host` concretos (sigue `Include`, ignora comodines,
  `Match` y el fichero generado `GENERATED_FILE_NAME`).
- Importaciones: `Db::import_hosts` en una transacción; `--dry-run` = misma transacción sin commit.
  Formato JSON = `model::HostData` (campos vacíos omitidos al serializar).

## Desarrollo
- `cargo test`, `cargo clippy --all-targets`.
- Pruebas manuales sin conectar: `SSHH_DB=/tmp/x.db SSHH_SSH=<script que imprima args> sshh ...`.
- `SSHH_DEBUG=1` muestra en stderr cómo se resuelve el destino y el comando que se ejecuta.
- El TUI se puede probar con tmux: `tmux new -d -s t -x 120 -y 30 sshh` + `send-keys` / `capture-pane -p`.
- `SSHH_INCLUDE_FILE` redirige el fichero generado; para probar `include_state` usar `HOME=<tmp>`
  (ojo: el `ssh` real lee el home de /etc/passwd, no `$HOME`; usar `ssh -F <config>`).
- Idioma de mensajes y comentarios: español.

## Roadmap
1. ✅ Base: DB, migraciones, parser ssh, exec.
2. ✅ TUI: lista, detalle, búsqueda fuzzy, filtro por tag, ratón.
3. ✅ Wizard para conexiones nuevas, CRUD en el TUI, tags, copiar comando.
4. ✅ Historial, import/export JSON, importar `~/.ssh/config`.
5. ✅ Fichero ssh_config generado; acciones rápidas (sftp, ssh-copy-id).
