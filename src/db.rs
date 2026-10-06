//! Almacenamiento en SQLite.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::model::{Host, HostData};

/// Migraciones en orden; `PRAGMA user_version` guarda cuántas se han aplicado.
const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE hosts (
    id            INTEGER PRIMARY KEY,
    alias         TEXT NOT NULL UNIQUE,
    hostname      TEXT NOT NULL,
    user          TEXT,
    port          INTEGER,
    identity_file TEXT,
    proxy_jump    TEXT,
    extra_options TEXT NOT NULL DEFAULT '[]',
    name          TEXT,
    description   TEXT,
    notes         TEXT,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);
CREATE TABLE tags (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
CREATE TABLE host_tags (
    host_id INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
    tag_id  INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (host_id, tag_id)
);
CREATE TABLE history (
    id           INTEGER PRIMARY KEY,
    host_id      INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
    connected_at INTEGER NOT NULL,
    args         TEXT NOT NULL
);
CREATE INDEX history_host ON history(host_id, connected_at);
"#];

const HOST_SELECT: &str = "
SELECT h.id, h.alias, h.hostname, h.user, h.port, h.identity_file, h.proxy_jump,
       h.extra_options, h.name, h.description, h.notes, h.created_at, h.updated_at,
       (SELECT MAX(connected_at) FROM history WHERE host_id = h.id) AS last_used,
       (SELECT COUNT(*) FROM history WHERE host_id = h.id) AS use_count
FROM hosts h";

pub struct Db {
    conn: Connection,
}

impl Db {
    /// Ruta de la base de datos: `$SSHH_DB` o `~/.local/share/sshh/sshh.db`.
    pub fn default_path() -> Result<PathBuf> {
        if let Some(path) = std::env::var_os("SSHH_DB") {
            return Ok(path.into());
        }
        let dirs = directories::ProjectDirs::from("", "", "sshh")
            .ok_or_else(|| anyhow!("no se pudo determinar el directorio de datos"))?;
        Ok(dirs.data_dir().join("sshh.db"))
    }

    pub fn open_default() -> Result<Self> {
        Self::open(&Self::default_path()?)
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
            && !dir.exists()
        {
            fs::create_dir_all(dir).with_context(|| format!("creando {}", dir.display()))?;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        let conn = Connection::open(path).with_context(|| format!("abriendo {}", path.display()))?;
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn insert_host(&mut self, data: &HostData) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let id = insert_in(&tx, data)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn update_host(&mut self, id: i64, data: &HostData) -> Result<()> {
        let tx = self.conn.transaction()?;
        update_in(&tx, id, data)?;
        tx.commit()?;
        Ok(())
    }

    /// Importa varias conexiones en una sola transacción. Con `dry_run` se
    /// hace todo igual pero se deshace al final.
    pub fn import_hosts(
        &mut self,
        hosts: &[HostData],
        on_conflict: OnConflict,
        dry_run: bool,
    ) -> Result<ImportReport> {
        let tx = self.conn.transaction()?;
        let mut report = ImportReport::default();
        for data in hosts {
            if let Err(e) = data.validate() {
                report.skipped.push((data.alias.clone(), format!("{e:#}")));
                continue;
            }
            let Some(existing) = find_alias_in(&tx, &data.alias)? else {
                insert_in(&tx, data)?;
                report.added.push(data.alias.clone());
                continue;
            };
            if same_data(&existing.data, data) {
                report.unchanged.push(data.alias.clone());
                continue;
            }
            match on_conflict {
                OnConflict::Skip => {
                    report.skipped.push((data.alias.clone(), "ya existe con otros datos".into()));
                }
                OnConflict::Overwrite => {
                    update_in(&tx, existing.id, data)?;
                    report.updated.push(data.alias.clone());
                }
                OnConflict::Rename => {
                    let alias = free_alias(&tx, &data.alias)?;
                    insert_in(&tx, &HostData { alias: alias.clone(), ..data.clone() })?;
                    report.renamed.push((data.alias.clone(), alias));
                }
            }
        }
        if !dry_run {
            tx.commit()?;
        }
        Ok(report)
    }

    pub fn delete_by_alias(&self, alias: &str) -> Result<bool> {
        Ok(self.conn.execute("DELETE FROM hosts WHERE alias = ?1", [alias])? > 0)
    }

    pub fn find_by_alias(&self, alias: &str) -> Result<Option<Host>> {
        find_alias_in(&self.conn, alias)
    }

    /// Busca una conexión guardada que apunte a `user@hostname:port`. Las
    /// conexiones sin usuario se comparan con `local_user` (lo que usaría ssh).
    pub fn find_by_target(
        &self,
        hostname: &str,
        user: &str,
        port: u16,
        local_user: &str,
    ) -> Result<Option<Host>> {
        let sql = format!(
            "{HOST_SELECT}
             WHERE h.hostname = ?1 COLLATE NOCASE AND COALESCE(h.user, ?4) = ?2
               AND COALESCE(h.port, 22) = ?3
             ORDER BY last_used DESC NULLS LAST, h.id LIMIT 1"
        );
        let host = self
            .conn
            .query_row(&sql, params![hostname, user, port, local_user], host_from_row)
            .optional()?;
        host.map(|h| with_tags(&self.conn, h)).transpose()
    }

    /// Todas las conexiones, las usadas más recientemente primero.
    pub fn list_hosts(&self) -> Result<Vec<Host>> {
        let sql = format!("{HOST_SELECT} ORDER BY last_used DESC NULLS LAST, h.alias");
        let mut stmt = self.conn.prepare(&sql)?;
        let hosts = stmt.query_map([], host_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        hosts.into_iter().map(|h| with_tags(&self.conn, h)).collect()
    }

    pub fn record_connection(&self, host_id: i64, args: &[String]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO history (host_id, connected_at, args) VALUES (?1, ?2, ?3)",
            params![host_id, now(), serde_json::to_string(args)?],
        )?;
        Ok(())
    }

    /// Últimas conexiones (de una conexión concreta o de todas), la más
    /// reciente primero.
    pub fn history(&self, host_id: Option<i64>, limit: usize) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT h.alias, hi.connected_at, hi.args
             FROM history hi JOIN hosts h ON h.id = hi.host_id
             WHERE ?1 IS NULL OR hi.host_id = ?1
             ORDER BY hi.connected_at DESC, hi.id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![host_id, limit as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
        })?;
        rows.map(|row| {
            let (alias, connected_at, args) = row?;
            Ok(HistoryEntry { alias, connected_at, args: serde_json::from_str(&args)? })
        })
        .collect()
    }
}

/// Qué hacer al importar una conexión cuyo alias ya existe con otros datos.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum OnConflict {
    /// No importarla
    #[default]
    Skip,
    /// Reemplazar la existente
    Overwrite,
    /// Importarla con otro alias (alias-2, alias-3…)
    Rename,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub unchanged: Vec<String>,
    /// (alias original, alias nuevo)
    pub renamed: Vec<(String, String)>,
    /// (alias, motivo)
    pub skipped: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub alias: String,
    pub connected_at: i64,
    pub args: Vec<String>,
}

fn insert_in(conn: &Connection, data: &HostData) -> Result<i64> {
    data.validate()?;
    let now = now();
    let inserted = conn.execute(
        "INSERT INTO hosts (alias, hostname, user, port, identity_file, proxy_jump,
                            extra_options, name, description, notes, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            data.alias,
            data.hostname,
            data.user,
            data.port,
            data.identity_file,
            data.proxy_jump,
            serde_json::to_string(&data.extra_options)?,
            data.name,
            data.description,
            data.notes,
            now,
        ],
    );
    check_alias_conflict(inserted, &data.alias)?;
    let id = conn.last_insert_rowid();
    set_tags(conn, id, &data.tags)?;
    Ok(id)
}

fn update_in(conn: &Connection, id: i64, data: &HostData) -> Result<()> {
    data.validate()?;
    let updated = conn.execute(
        "UPDATE hosts SET alias = ?2, hostname = ?3, user = ?4, port = ?5, identity_file = ?6,
                          proxy_jump = ?7, extra_options = ?8, name = ?9, description = ?10,
                          notes = ?11, updated_at = ?12
         WHERE id = ?1",
        params![
            id,
            data.alias,
            data.hostname,
            data.user,
            data.port,
            data.identity_file,
            data.proxy_jump,
            serde_json::to_string(&data.extra_options)?,
            data.name,
            data.description,
            data.notes,
            now(),
        ],
    );
    if check_alias_conflict(updated, &data.alias)? == 0 {
        bail!("la conexión ya no existe");
    }
    set_tags(conn, id, &data.tags)
}

fn find_alias_in(conn: &Connection, alias: &str) -> Result<Option<Host>> {
    let sql = format!("{HOST_SELECT} WHERE h.alias = ?1");
    let host = conn.query_row(&sql, [alias], host_from_row).optional()?;
    host.map(|h| with_tags(conn, h)).transpose()
}

fn with_tags(conn: &Connection, mut host: Host) -> Result<Host> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.name FROM tags t JOIN host_tags ht ON ht.tag_id = t.id
         WHERE ht.host_id = ?1 ORDER BY t.name",
    )?;
    host.data.tags = stmt
        .query_map([host.id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(host)
}

/// Compara ignorando el orden de los tags.
fn same_data(a: &HostData, b: &HostData) -> bool {
    let sorted = |d: &HostData| {
        let mut tags = d.tags.clone();
        tags.sort();
        tags.dedup();
        HostData { tags, ..d.clone() }
    };
    sorted(a) == sorted(b)
}

/// Primer `alias-N` libre.
fn free_alias(conn: &Connection, alias: &str) -> Result<String> {
    for n in 2.. {
        let candidate = format!("{alias}-{n}");
        if find_alias_in(conn, &candidate)?.is_none() {
            return Ok(candidate);
        }
    }
    unreachable!()
}

/// Traduce la violación de `UNIQUE(alias)` a un error legible.
fn check_alias_conflict(result: rusqlite::Result<usize>, alias: &str) -> Result<usize> {
    if let Err(rusqlite::Error::SqliteFailure(e, _)) = &result
        && e.code == rusqlite::ErrorCode::ConstraintViolation
    {
        bail!("ya existe una conexión con el alias «{alias}»");
    }
    Ok(result?)
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let applied: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let applied = usize::try_from(applied)?;
    if applied > MIGRATIONS.len() {
        bail!("la base de datos es de una versión más reciente de sshh");
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(applied) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)
            .with_context(|| format!("aplicando la migración {}", i + 1))?;
        tx.pragma_update(None, "user_version", i64::try_from(i + 1)?)?;
        tx.commit()?;
    }
    Ok(())
}

fn set_tags(conn: &Connection, host_id: i64, tags: &[String]) -> Result<()> {
    conn.execute("DELETE FROM host_tags WHERE host_id = ?1", [host_id])?;
    for tag in tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        conn.execute("INSERT INTO tags (name) VALUES (?1) ON CONFLICT(name) DO NOTHING", [tag])?;
        conn.execute(
            "INSERT OR IGNORE INTO host_tags (host_id, tag_id)
             SELECT ?1, id FROM tags WHERE name = ?2",
            params![host_id, tag],
        )?;
    }
    Ok(())
}

fn host_from_row(row: &Row) -> rusqlite::Result<Host> {
    let extra: String = row.get("extra_options")?;
    let extra_options = serde_json::from_str(&extra).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(Host {
        id: row.get("id")?,
        data: HostData {
            alias: row.get("alias")?,
            hostname: row.get("hostname")?,
            user: row.get("user")?,
            port: row.get("port")?,
            identity_file: row.get("identity_file")?,
            proxy_jump: row.get("proxy_jump")?,
            extra_options,
            name: row.get("name")?,
            description: row.get("description")?,
            notes: row.get("notes")?,
            tags: Vec::new(),
        },
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        last_used: row.get("last_used")?,
        use_count: row.get("use_count")?,
    })
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SshOption;

    fn sample(alias: &str) -> HostData {
        HostData {
            alias: alias.into(),
            hostname: "10.0.0.1".into(),
            user: Some("root".into()),
            extra_options: vec![SshOption { key: "ForwardAgent".into(), value: "yes".into() }],
            tags: vec!["prod".into(), "db".into()],
            ..Default::default()
        }
    }

    #[test]
    fn insert_and_find() {
        let mut db = Db::open_in_memory().unwrap();
        db.insert_host(&sample("db1")).unwrap();

        let host = db.find_by_alias("db1").unwrap().unwrap();
        assert_eq!(host.data.tags, ["db", "prod"]);
        assert_eq!(host.data.extra_options.len(), 1);
        assert_eq!(host.use_count, 0);

        assert!(db.find_by_target("10.0.0.1", "root", 22, "me").unwrap().is_some());
        assert!(db.find_by_target("10.0.0.1", "other", 22, "me").unwrap().is_none());
        assert!(db.find_by_target("10.0.0.1", "root", 2222, "me").unwrap().is_none());
        assert!(db.find_by_alias("nope").unwrap().is_none());
    }

    #[test]
    fn target_without_user_matches_local_user() {
        let mut db = Db::open_in_memory().unwrap();
        db.insert_host(&HostData { user: None, hostname: "Web.Example.com".into(), ..sample("w") })
            .unwrap();
        assert!(db.find_by_target("web.example.com", "me", 22, "me").unwrap().is_some());
        assert!(db.find_by_target("web.example.com", "root", 22, "me").unwrap().is_none());
    }

    #[test]
    fn update_host() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_host(&sample("a")).unwrap();
        db.insert_host(&sample("b")).unwrap();

        let data = HostData { alias: "a2".into(), tags: vec!["x".into()], ..sample("") };
        db.update_host(a, &data).unwrap();
        let host = db.find_by_alias("a2").unwrap().unwrap();
        assert_eq!((host.id, host.data.tags), (a, vec!["x".to_string()]));

        let err = db.update_host(a, &HostData { alias: "b".into(), ..data.clone() }).unwrap_err();
        assert!(err.to_string().contains("ya existe"));
        assert!(db.update_host(999, &data).is_err());
    }

    #[test]
    fn duplicate_alias_is_rejected() {
        let mut db = Db::open_in_memory().unwrap();
        db.insert_host(&sample("a")).unwrap();
        let err = db.insert_host(&sample("a")).unwrap_err();
        assert!(err.to_string().contains("ya existe"));
    }

    #[test]
    fn history_orders_list() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_host(&sample("a")).unwrap();
        db.insert_host(&sample("b")).unwrap();
        db.record_connection(a, &["a".into()]).unwrap();

        let hosts = db.list_hosts().unwrap();
        assert_eq!(hosts[0].data.alias, "a");
        assert_eq!(hosts[0].use_count, 1);
        assert!(hosts[0].last_used.is_some());
        assert!(hosts[1].last_used.is_none());
    }

    #[test]
    fn import_strategies() {
        let mut db = Db::open_in_memory().unwrap();
        db.insert_host(&sample("a")).unwrap();
        let changed = HostData { hostname: "otro".into(), ..sample("a") };
        let mut reordered = sample("a");
        reordered.tags.reverse();
        let invalid = HostData { alias: "con espacio".into(), ..sample("") };
        let batch = [changed.clone(), sample("b"), invalid];

        let r = db.import_hosts(&batch, OnConflict::Skip, false).unwrap();
        assert_eq!(r.added, ["b"]);
        assert_eq!(r.skipped.len(), 2);

        let r = db.import_hosts(&[reordered], OnConflict::Skip, false).unwrap();
        assert_eq!(r.unchanged, ["a"]);

        let r = db.import_hosts(std::slice::from_ref(&changed), OnConflict::Rename, false).unwrap();
        assert_eq!(r.renamed, [("a".to_string(), "a-2".to_string())]);

        let r = db.import_hosts(&[changed], OnConflict::Overwrite, false).unwrap();
        assert_eq!(r.updated, ["a"]);
        assert_eq!(db.find_by_alias("a").unwrap().unwrap().data.hostname, "otro");
    }

    #[test]
    fn import_dry_run_rolls_back() {
        let mut db = Db::open_in_memory().unwrap();
        let r = db.import_hosts(&[sample("a")], OnConflict::Skip, true).unwrap();
        assert_eq!(r.added, ["a"]);
        assert!(db.list_hosts().unwrap().is_empty());
    }

    #[test]
    fn history_queries() {
        let mut db = Db::open_in_memory().unwrap();
        let a = db.insert_host(&sample("a")).unwrap();
        let b = db.insert_host(&sample("b")).unwrap();
        db.record_connection(a, &["a".into()]).unwrap();
        db.record_connection(b, &["-v".into(), "b".into()]).unwrap();
        db.record_connection(a, &["a".into(), "uptime".into()]).unwrap();

        let all = db.history(None, 10).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].args, ["a", "uptime"]);
        let only_b = db.history(Some(b), 10).unwrap();
        assert_eq!((only_b.len(), only_b[0].alias.as_str()), (1, "b"));
        assert_eq!(db.history(None, 2).unwrap().len(), 2);
    }

    #[test]
    fn delete_cascades() {
        let mut db = Db::open_in_memory().unwrap();
        let id = db.insert_host(&sample("a")).unwrap();
        db.record_connection(id, &[]).unwrap();
        assert!(db.delete_by_alias("a").unwrap());
        assert!(!db.delete_by_alias("a").unwrap());
        let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM history", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn migrations_are_idempotent() {
        let dir = std::env::temp_dir().join(format!("sshh-test-{}", std::process::id()));
        let path = dir.join("t.db");
        Db::open(&path).unwrap();
        Db::open(&path).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }
}
