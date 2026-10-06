# sshh

Gestor de conexiones SSH para la terminal, escrito en Rust.

- Se usa **igual que `ssh`**: `sshh usuario@host`, `sshh -p 2222 -J bastion web`, etc.
- Si la conexión es nueva, aparece un **wizard** para guardarla con nombre, descripción, tags y notas.
- Sin argumentos abre un **TUI** con búsqueda fuzzy, navegación por teclado (estilo vim) y ratón.
- **No reimplementa SSH**: siempre ejecuta el `ssh` del sistema, así que funcionan tu `~/.ssh/config`,
  `ssh-agent`, `ProxyJump`, `ControlMaster`, `known_hosts`, claves FIDO, etc.
- **No guarda contraseñas** ni secretos: usa tus claves y tu agente como siempre.

> Estado: en desarrollo. Ver [Roadmap](#roadmap).

## Índice

- [Instalación](#instalación)
- [Uso rápido](#uso-rápido)
- [Modo wrapper: `sshh` como `ssh`](#modo-wrapper-sshh-como-ssh)
- [Wizard de conexiones nuevas](#wizard-de-conexiones-nuevas)
- [TUI](#tui)
- [Formulario de conexión](#formulario-de-conexión)
- [Subcomandos](#subcomandos)
- [Importar `~/.ssh/config`](#importar-sshconfig)
- [Exportar e importar (JSON)](#exportar-e-importar-json)
- [Historial](#historial)
- [Integración con ssh, scp, rsync, git…](#integración-con-ssh-scp-rsync-git)
- [Datos y almacenamiento](#datos-y-almacenamiento)
- [Variables de entorno](#variables-de-entorno)
- [Desarrollo](#desarrollo)
- [Roadmap](#roadmap)

## Instalación

Requisitos: Rust ≥ 1.88 (edición 2024) y OpenSSH instalado. SQLite va incluido en el binario.

```sh
cargo build --release
install -Dm755 target/release/sshh ~/.local/bin/sshh
```

Opcionalmente, para usarlo siempre en lugar de `ssh`:

```sh
# ~/.bashrc / ~/.zshrc
alias ssh=sshh
```

Lo que `sshh` no entiende lo pasa tal cual a `ssh`, así que el alias es seguro. Si llegas a instalar
`sshh` con el nombre `ssh` en el `PATH`, detecta que es él mismo y busca el `ssh` real.

## Uso rápido

```sh
sshh                          # abre el TUI
sshh root@10.0.0.5            # conecta; si es nueva, ofrece guardarla
sshh web                      # conecta a la conexión guardada con alias «web»
sshh web uptime               # comando remoto, como con ssh
sshh -L 8080:localhost:80 web # cualquier opción de ssh funciona
sshh ls                       # lista las conexiones guardadas
sshh import-ssh-config        # importa los Host de tu ~/.ssh/config
sshh history                  # últimas conexiones
```

## Modo wrapper: `sshh` como `ssh`

`sshh [opciones de ssh] destino [comando]` acepta exactamente los mismos argumentos que `ssh`
(incluido `ssh://user@host:port`). Según el destino:

| Caso | Qué hace |
|---|---|
| El destino es el **alias** de una conexión guardada | Añade antes del destino `-o HostName=… -o User=… -o Port=…` (y identidad, ProxyJump y opciones extra) **solo para lo que no hayas indicado tú**. Tus flags (`-p`, `-l`, `-i`, `-J`, `-o`) siempre ganan. |
| El destino **coincide** con una conexión guardada (mismo `usuario@host:puerto`) | Ejecuta `ssh` con tus argumentos intactos y lo apunta en el historial. |
| El destino **no está guardado** | Abre el [wizard](#wizard-de-conexiones-nuevas) (solo en terminal interactiva). |
| Sin destino (`-V`, `-Q cipher`…) o argumentos que no entiende | Los pasa tal cual a `ssh`. |

Para saber si un destino ya está guardado, `sshh` pregunta a `ssh -G` a dónde conectaría realmente,
aplicando tu `~/.ssh/config`. Así `sshh ovtest` (un `Host` de tu config) se reconoce igual que
`sshh somadmin@10.50.1.17 -p 2200`.

Las llamadas que solo consultan (`-G`, `-V`, `-O`, `-Q`) no abren el wizard ni cuentan en el historial.

`sshh` nunca impide una conexión: si falla algo propio (base de datos, parser), lo avisa y ejecuta
`ssh` con tus argumentos originales.

Para ver qué decide y qué ejecuta exactamente:

```sh
SSHH_DEBUG=1 sshh web
# sshh[debug]: «web» es el alias de una conexión guardada
# sshh[debug]: exec /usr/bin/ssh -o HostName=10.0.0.5 -o User=deploy -o Port=2222 web
```

Para conectar a un host que se llame igual que un subcomando (`ls`, `add`, `rm`, `help`, `history`,
`export`, `import`, `import-ssh-config`, `ssh-config`), usa `sshh -- ls`.

## Wizard de conexiones nuevas

Al conectar a un destino que no está guardado aparece un formulario prerrellenado con lo que diría
`ssh -G`: host, usuario, puerto y ProxyJump. El alias propuesto es el nombre corto del host
(`web.example.com` → `web`; las IPs se quedan igual).

| Tecla | Acción |
|---|---|
| `Enter` | guardar y conectar |
| `Esc` | conectar sin guardar |
| `Ctrl-c` | cancelar (no conecta) |

Solo aparece si stdin, stdout y stderr son una terminal, así que no molesta en scripts. Se puede
desactivar con `SSHH_NO_WIZARD=1`.

## TUI

`sshh` sin argumentos abre la lista de conexiones, un panel de detalle (con las notas y las últimas
conexiones de la seleccionada) y una barra de atajos. La lista se puede ordenar por uso reciente, por
número de usos o por alias. Si la salida no es una terminal (`sshh | less`), imprime la lista en
texto.

### Teclado

Los atajos siguen las convenciones de vim, less, lazygit y fzf.

| Tecla | Acción |
|---|---|
| `Enter` | conectar |
| `j` / `k`, `↓` / `↑` | mover |
| `gg` / `G` | ir al principio / al final |
| `Ctrl-d` / `Ctrl-u` | media página abajo / arriba |
| `Ctrl-f` / `Ctrl-b`, `PgDn` / `PgUp` | página abajo / arriba |
| `Tab` | foco lista ↔ detalle (para hacer scroll en notas largas) |
| `/` | buscar |
| `a` | nueva conexión |
| `e` | editar |
| `t` | editar tags |
| `dd` | borrar (pide confirmación con `y`) |
| `yy` | copiar el comando `ssh` equivalente al portapapeles |
| `s` | abrir `sftp` con la conexión |
| `c` | instalar tu clave pública en el servidor (`ssh-copy-id`) |
| `o` | cambiar el orden: recientes → más usadas → alfabético |
| `R` | recargar la lista |
| `?` | ayuda |
| `Esc` | volver un nivel / limpiar la búsqueda (nunca sale) |
| `q`, `Ctrl-c` | salir |

### Búsqueda

`/` activa la búsqueda **fuzzy** sobre alias, nombre, usuario, host, descripción y tags. Las letras que
coinciden se resaltan.

- `#tag` filtra por tag (por prefijo): `#prod`, `#prod web`, `#prod #db`.
- `Enter` conecta con el resultado seleccionado (como en fzf).
- `↑` / `↓`, `Ctrl-j` / `Ctrl-k` o `Ctrl-n` / `Ctrl-p` mueven sin salir de la búsqueda.
- `Ctrl-w` borra una palabra; `Ctrl-u` borra todo.
- `Esc` vuelve a la lista manteniendo el filtro; un segundo `Esc` lo limpia.

### Ratón

| Acción | Efecto |
|---|---|
| Click en una fila | seleccionar |
| Doble click | conectar |
| Rueda | mover (o scroll si el cursor está sobre el detalle) |
| Click en el cuadro de búsqueda | buscar |
| Click en el detalle | darle el foco |
| Click en un campo del formulario | editar ese campo |

## Formulario de conexión

El mismo formulario se usa para crear (`a`), editar (`e`, `t`) y en el wizard.

| Campo | Notas |
|---|---|
| Alias | Obligatorio y único. Es lo que usas en `sshh <alias>`. Sin espacios ni `* ? ! , @ " #`. |
| Host | Obligatorio. Hostname o IP. |
| Usuario, Puerto | Opcionales. Vacío = lo que decida ssh (usuario local, 22). |
| Identidad | Ruta a la clave (`IdentityFile`). |
| ProxyJump | `bastion` o `user@host:port`. |
| Nombre, Descripción | Texto libre para identificarla. |
| Tags | Separados por comas o espacios; el `#` inicial es opcional. |
| Opciones | Opciones extra de ssh_config, una por línea: `ForwardAgent=yes`. |
| Notas | Texto multilínea libre. |

| Tecla | Acción |
|---|---|
| `Tab` / `Shift-Tab`, `↓` / `↑` | campo siguiente / anterior (en campos multilínea, las flechas recorren líneas) |
| `Enter` | guardar (en Opciones y Notas inserta un salto de línea) |
| `Ctrl-s` | guardar desde cualquier campo |
| `Ctrl-e` | editar Opciones o Notas en `$VISUAL` / `$EDITOR` (o `vi`) |
| `Ctrl-w` / `Ctrl-u` | borrar palabra / hasta el inicio de línea |
| `Home` / `Ctrl-a`, `End` | inicio / fin de línea |
| `Esc` | cancelar (en el wizard: conectar sin guardar) |

Si un dato no es válido (puerto incorrecto, alias repetido…), el foco salta a ese campo y el error
aparece en rojo.

## Subcomandos

Además del TUI, hay subcomandos para scripts o uso rápido:

```sh
sshh ls                                   # lista en texto
sshh add <alias> <destino> [opciones]     # guardar una conexión
sshh rm <alias>                           # borrar una conexión
sshh history [alias] [-n N]               # últimas conexiones
sshh export [-o fichero]                  # exportar a JSON
sshh import <fichero|->                   # importar desde JSON
sshh import-ssh-config [fichero]          # importar los Host de ~/.ssh/config
sshh ssh-config [status|sync|print|disable]  # fichero ssh_config generado (ver abajo)
sshh --help                               # ayuda (también `sshh <subcomando> --help`)
```

Opciones de `sshh add`:

| Opción | Descripción |
|---|---|
| `-p, --port <PUERTO>` | puerto |
| `-i, --identity <RUTA>` | clave (`IdentityFile`) |
| `-J, --jump <DESTINO>` | ProxyJump |
| `-o, --option <Clave=Valor>` | opción extra de ssh_config (repetible) |
| `-n, --name <TEXTO>` | nombre |
| `-d, --description <TEXTO>` | descripción |
| `--notes <TEXTO>` | notas |
| `-t, --tag <TAGS>` | tags separados por comas (repetible) |

```sh
sshh add web deploy@10.0.0.5 -p 2222 -i ~/.ssh/deploy -n "Web producción" -t prod,web \
  -o ForwardAgent=yes --notes "Reiniciar: sudo systemctl restart nginx"
```

## Importar `~/.ssh/config`

```sh
sshh import-ssh-config --dry-run          # ver qué se importaría
sshh import-ssh-config -t ssh-config      # importar añadiendo el tag «ssh-config»
sshh import-ssh-config otra/config        # otro fichero
```

- Se importa cada bloque `Host` con nombres concretos. `Host web1 web2` crea dos conexiones.
- `HostName`, `User`, `Port`, `IdentityFile` y `ProxyJump` van a sus campos; el resto de opciones
  (`LocalForward`, `ForwardAgent`, más `IdentityFile`…) se guardan como opciones extra.
- Sin `HostName`, el host es el propio alias (igual que hace ssh).
- Los comentarios justo encima de un `Host` pasan a ser sus notas.
- Si un alias aparece en varios bloques, se combinan y gana el primer valor de cada opción, como en ssh.
- Se siguen los `Include` (con rutas relativas a `~/.ssh` y comodines como `config.d/*`).
- **No se importan** los `Host` con comodines o negaciones (`Host *`, `Host *.lan`, `!foo`), los bloques
  `Match` ni las opciones globales: ssh los sigue aplicando igualmente al conectar. El comando los lista
  al final para que sepas cuáles son.

Opciones comunes a `import` e `import-ssh-config`:

| Opción | Descripción |
|---|---|
| `-n, --dry-run` | muestra qué pasaría sin guardar nada |
| `--on-conflict skip` | (por defecto) si el alias ya existe con otros datos, no la importa |
| `--on-conflict overwrite` | reemplaza la existente |
| `--on-conflict rename` | la importa con otro alias (`web-2`, `web-3`…) |

Las conexiones que ya existen con los mismos datos se cuentan como «sin cambios», así que importar
dos veces no duplica nada.

```text
$ sshh import-ssh-config
3 nuevas
  + ovtest
  + stcas1
  + nusakan
```

## Exportar e importar (JSON)

```sh
sshh export -o conexiones.json            # se crea con permisos 0600 (las notas pueden ser sensibles)
sshh export > conexiones.json
sshh import conexiones.json
sshh import --on-conflict rename conexiones.json
cat conexiones.json | sshh import -
```

El export incluye todos los datos de cada conexión, pero no el historial. Formato:

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
      "name": "Web producción",
      "description": "Frontend nginx",
      "notes": "Reiniciar: sudo systemctl restart nginx",
      "tags": ["prod", "web"]
    }
  ]
}
```

Solo `alias` y `hostname` son obligatorios. `sshh import` también acepta directamente una lista
`[{...}, {...}]`, lo que facilita generar el fichero desde otras herramientas. Las entradas no válidas
se omiten con su motivo y el resto se importa.

## Historial

Cada conexión que pasa por `sshh` a un host guardado queda registrada con la fecha y los argumentos.

```text
$ sshh history
FECHA             ALIAS   COMANDO
2026-10-07 13:27  web     sshh web
2026-10-07 13:27  ovtest  sshh -v ovtest
2026-10-07 13:27  web     sshh web uptime
```

- `sshh history web` muestra solo las de una conexión; `-n 50` cambia el límite (20 por defecto).
- En el TUI, el detalle muestra las 5 últimas de la conexión seleccionada, y la lista se puede ordenar
  por uso reciente o por número de usos (`o`).
- No se registran las llamadas que solo consultan (`-G`, `-V`, `-O`, `-Q`) ni las conexiones a
  destinos no guardados.

## Integración con ssh, scp, rsync, git…

`sshh` puede mantener un fichero `~/.ssh/config.d/sshh.conf` con un bloque `Host` por cada conexión
guardada. Al incluirlo desde tu `~/.ssh/config`, **cualquier herramienta** que use ssh conoce tus
alias aunque no pase por `sshh`: `ssh web`, `scp fichero web:`, `rsync -a dir/ web:`,
`git clone web:repo`, VS Code Remote-SSH, Ansible…

**`sshh` nunca modifica tu `~/.ssh/config`.** Solo escribe su propio fichero; la línea que lo incluye
la añades tú.

### Activarlo

1. Añade esta línea **al principio** de `~/.ssh/config`, antes de cualquier `Host` o `Match`:

   ```sshconfig
   Include config.d/sshh.conf
   ```

   Tiene que ir arriba: en ssh_config, un `Include` escrito después de un bloque `Host` forma parte
   de ese bloque y solo se aplicaría a ese host. La ruta relativa se resuelve desde `~/.ssh`. Si el
   fichero aún no existe, ssh simplemente lo ignora, así que puedes añadir la línea antes del paso 2.

2. Genera el fichero:

   ```sh
   sshh ssh-config sync
   ```

3. Comprueba que todo está bien:

   ```sh
   sshh ssh-config            # estado: activado y bien incluido
   ssh -G web | head          # ssh ya resuelve el alias
   ```

Desde ese momento el fichero se regenera solo cada vez que guardas, editas, borras o importas
conexiones (desde el TUI, el wizard o la CLI).

| Comando | Qué hace |
|---|---|
| `sshh ssh-config` / `status` | dice si está activado y si `~/.ssh/config` lo incluye (y si la línea está mal colocada) |
| `sshh ssh-config sync` | genera el fichero ahora y lo activa |
| `sshh ssh-config print` | muestra lo que se generaría, sin escribir nada |
| `sshh ssh-config disable` | borra el fichero y deja de actualizarlo (la línea `Include` puede quedarse) |

Ejemplo de fichero generado (permisos `0600`, escrito de forma atómica). Las notas **no** se incluyen:

```sshconfig
# Generado por sshh: no lo edites, se sobrescribe con cada cambio.

# Web producción — Frontend nginx
Host web
    HostName 10.0.0.5
    User deploy
    Port 2222
    IdentityFile ~/.ssh/deploy
    LocalForward 8080 localhost:80
```

**Si un alias está a la vez en tu `~/.ssh/config` y en `sshh`** (por ejemplo, tras
`sshh import-ssh-config`), con el `Include` arriba gana el valor de `sshh` para cada opción que defina,
porque ssh se queda con el primer valor que encuentra. Las opciones que solo estén en tu config se
siguen aplicando. Si quieres que la fuente de verdad sea `sshh`, puedes borrar esos bloques de tu
config. `sshh import-ssh-config` nunca importa el fichero generado.

## Datos y almacenamiento

- Base de datos SQLite en `~/.local/share/sshh/sshh.db` (directorio con permisos `0700`).
- Se guardan: alias, host, usuario, puerto, identidad, ProxyJump, opciones extra, nombre, descripción,
  notas, tags e historial de conexiones (fecha y argumentos).
- **No se guardan contraseñas ni claves.**
- El esquema se migra solo al arrancar.

- Opcionalmente, `~/.ssh/config.d/sshh.conf` (ver [Integración](#integración-con-ssh-scp-rsync-git)),
  que se genera a partir de la base de datos: la fuente de verdad sigue siendo SQLite.

## Variables de entorno

| Variable | Efecto |
|---|---|
| `SSHH_DEBUG=1` | muestra en stderr cómo se resuelve el destino y el comando exacto que se ejecuta |
| `SSHH_NO_WIZARD=1` | no abre el wizard con destinos nuevos |
| `SSHH_DB=<ruta>` | usa otra base de datos (útil para pruebas) |
| `SSHH_SSH=<ruta>` | usa otro binario en lugar de `ssh` |
| `SSHH_INCLUDE_FILE=<ruta>` | otra ruta para el fichero ssh_config generado (útil para pruebas) |
| `VISUAL` / `EDITOR` | editor para `Ctrl-e` en el formulario |

Para el portapapeles (`yy`) se usa `wl-copy` (Wayland) o `xclip` (X11) si están disponibles y, si
no, la secuencia OSC 52, que soportan la mayoría de terminales y que también funciona por ssh.

## Desarrollo

```sh
cargo test
cargo clippy --all-targets
```

Probar sin conectar a ningún sitio: apunta `SSHH_SSH` a un script que imprima sus argumentos y usa
una base de datos aparte.

```sh
export SSHH_DB=/tmp/prueba.db SSHH_SSH=echo SSHH_DEBUG=1
cargo run -- add web deploy@10.0.0.5 -p 2222
cargo run -- web uptime
```

El TUI se puede probar de forma automatizada con tmux:

```sh
tmux new -d -s t -x 120 -y 30 target/debug/sshh
tmux send-keys -t t / w e b
tmux capture-pane -p -t t
```

### Estructura

| Módulo | Responsabilidad |
|---|---|
| `src/main.rs` | punto de entrada: TUI, subcomando o modo wrapper |
| `src/ssh_args.rs` | parser de la línea de comandos de OpenSSH |
| `src/connect.rs` | modo wrapper: resolución del destino (`ssh -G`), wizard, `exec` de ssh |
| `src/db.rs` | SQLite, migraciones (`PRAGMA user_version`), importación e historial |
| `src/model.rs` | modelo de datos y validación |
| `src/ssh_config.rs` | lectura de ficheros ssh_config (con `Include`) para importarlos |
| `src/include.rs` | fichero ssh_config generado (`~/.ssh/config.d/sshh.conf`) |
| `src/cli.rs` | subcomandos (`ls`, `add`, `rm`, `history`, `export`, `import`, `import-ssh-config`, `ssh-config`) |
| `src/tui/app.rs` | estado y manejo de eventos del TUI (sin terminal; testeable) |
| `src/tui/form.rs` | formulario de alta, edición y wizard |
| `src/tui/ui.rs` | renderizado |
| `src/tui/term.rs` | terminal, `$EDITOR` y portapapeles |

## Roadmap

- [x] **Fase 1**: base de datos, migraciones, parser de argumentos de ssh y `exec`.
- [x] **Fase 2**: TUI con lista, detalle, búsqueda fuzzy, filtro por tag y ratón.
- [x] **Fase 3**: wizard de conexiones nuevas, alta, edición y borrado en el TUI, tags y copiar comando.
- [x] **Fase 4**: historial, exportar/importar JSON, importar `~/.ssh/config`.
- [x] **Fase 5**: fichero ssh_config generado para el `Include` y acciones rápidas (sftp, ssh-copy-id).

Ideas pendientes: «no volver a preguntar» por un destino en el wizard, `sshh add` sin argumentos
abriendo el formulario, atajos configurables.
