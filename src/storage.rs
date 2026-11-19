use crate::crypto::{self, Key};
use crate::model::*;
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use crate::i18n::{Key as TKey, t};

pub struct Storage {
    db: Connection,
    /// Ключ шифрования паролей; None — мастер-пароль не задан или база ещё не разблокирована.
    key: Option<Key>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lock {
    NoPassword,
    Locked,
    Unlocked,
}

/// Известная строка, зашифрованная ключом, — для проверки мастер-пароля.
const CHECK: &str = "pglistener";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS folders (
    id        INTEGER PRIMARY KEY,
    parent_id INTEGER,
    name      TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS connections (
    id                 INTEGER PRIMARY KEY,
    folder_id          INTEGER,
    name               TEXT NOT NULL,
    host               TEXT NOT NULL DEFAULT '',
    port               INTEGER NOT NULL DEFAULT 5432,
    dbname             TEXT NOT NULL DEFAULT '',
    username           TEXT NOT NULL DEFAULT '',
    password           TEXT NOT NULL DEFAULT '',
    sslmode            TEXT NOT NULL DEFAULT 'prefer',
    log_dir            TEXT NOT NULL DEFAULT '',
    ssh_enabled        INTEGER NOT NULL DEFAULT 0,
    ssh_host           TEXT NOT NULL DEFAULT '',
    ssh_port           INTEGER NOT NULL DEFAULT 22,
    ssh_user           TEXT NOT NULL DEFAULT '',
    ssh_auth           TEXT NOT NULL DEFAULT 'password',
    ssh_password       TEXT NOT NULL DEFAULT '',
    ssh_key_path       TEXT NOT NULL DEFAULT '',
    ssh_key_passphrase TEXT NOT NULL DEFAULT '',
    filters            TEXT NOT NULL DEFAULT '',
    active             INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS channels (
    connection_id INTEGER NOT NULL,
    name          TEXT NOT NULL,
    PRIMARY KEY (connection_id, name)
);
";

impl Storage {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch(SCHEMA)?;
        // Миграция баз, созданных старыми версиями: добавляем недостающие колонки.
        for (column, ddl) in [
            ("include_filters", "include_filters TEXT NOT NULL DEFAULT ''"),
            ("bridge_enabled", "bridge_enabled INTEGER NOT NULL DEFAULT 0"),
            ("bridge_listen", "bridge_listen TEXT NOT NULL DEFAULT '0.0.0.0'"),
            ("bridge_port", "bridge_port INTEGER NOT NULL DEFAULT 15432"),
            ("bridge_allow", "bridge_allow TEXT NOT NULL DEFAULT ''"),
        ] {
            let exists = db
                .prepare("SELECT 1 FROM pragma_table_info('connections') WHERE name = ?1")?
                .exists([column])?;
            if !exists {
                db.execute_batch(&format!("ALTER TABLE connections ADD COLUMN {ddl}"))?;
            }
        }
        Ok(Self { db, key: None })
    }

    pub fn folders(&self) -> Result<Vec<Folder>> {
        let mut st = self
            .db
            .prepare("SELECT id, parent_id, name FROM folders ORDER BY name COLLATE NOCASE")?;
        let rows = st.query_map([], |r| {
            Ok(Folder {
                id: r.get(0)?,
                parent_id: r.get(1)?,
                name: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn connections(&self) -> Result<Vec<ConnConfig>> {
        let mut st = self.db.prepare(
            "SELECT id, folder_id, name, host, port, dbname, username, password, sslmode, log_dir,
                    ssh_enabled, ssh_host, ssh_port, ssh_user, ssh_auth, ssh_password, ssh_key_path,
                    ssh_key_passphrase, filters, active, include_filters,
                    bridge_enabled, bridge_listen, bridge_port, bridge_allow
             FROM connections ORDER BY name COLLATE NOCASE",
        )?;
        let rows = st.query_map([], |r| {
            let split = |s: String| -> Vec<String> {
                s.split('\n').filter(|s| !s.is_empty()).map(String::from).collect()
            };
            let filters: String = r.get(18)?;
            let includes: String = r.get(20)?;
            Ok(ConnConfig {
                id: r.get(0)?,
                folder_id: r.get(1)?,
                name: r.get(2)?,
                host: r.get(3)?,
                port: r.get(4)?,
                dbname: r.get(5)?,
                user: r.get(6)?,
                password: r.get(7)?,
                sslmode: SslMode::parse(&r.get::<_, String>(8)?),
                log_dir: r.get(9)?,
                ssh: SshConfig {
                    enabled: r.get(10)?,
                    host: r.get(11)?,
                    port: r.get(12)?,
                    user: r.get(13)?,
                    auth: SshAuth::parse(&r.get::<_, String>(14)?),
                    password: r.get(15)?,
                    key_path: r.get(16)?,
                    key_passphrase: r.get(17)?,
                },
                filters: split(filters),
                includes: split(includes),
                bridge: BridgeConfig {
                    enabled: r.get(21)?,
                    listen: r.get(22)?,
                    port: r.get(23)?,
                    allow: split(r.get(24)?),
                },
                channels: Vec::new(),
                active: r.get(19)?,
            })
        })?;
        let mut list: Vec<ConnConfig> = rows.collect::<Result<_, _>>()?;
        for c in &mut list {
            for v in [&mut c.password, &mut c.ssh.password, &mut c.ssh.key_passphrase] {
                *v = match &self.key {
                    Some(k) => k.decrypt(v).unwrap_or_default(),
                    None if crypto::is_encrypted(v) => String::new(),
                    None => std::mem::take(v),
                };
            }
        }
        let mut st = self
            .db
            .prepare("SELECT name FROM channels WHERE connection_id = ?1 ORDER BY name")?;
        for c in &mut list {
            c.channels = st
                .query_map([c.id], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
        }
        Ok(list)
    }

    /// Сохраняет подключение (вставка при id == 0) вместе со списком каналов. Возвращает id.
    pub fn save_connection(&self, c: &ConnConfig) -> Result<i64> {
        let filters = c.filters.join("\n");
        let includes = c.includes.join("\n");
        let bridge_allow = c.bridge.allow.join("\n");
        let secret = |v: &str| match &self.key {
            Some(k) => k.encrypt(v),
            None => v.to_string(),
        };
        let (password, ssh_password, ssh_passphrase) = (
            secret(&c.password),
            secret(&c.ssh.password),
            secret(&c.ssh.key_passphrase),
        );
        let p = params![
            c.folder_id,
            c.name,
            c.host,
            c.port,
            c.dbname,
            c.user,
            password,
            c.sslmode.as_str(),
            c.log_dir,
            c.ssh.enabled,
            c.ssh.host,
            c.ssh.port,
            c.ssh.user,
            c.ssh.auth.as_str(),
            ssh_password,
            c.ssh.key_path,
            ssh_passphrase,
            filters,
            c.active,
            includes,
            c.bridge.enabled,
            c.bridge.listen,
            c.bridge.port,
            bridge_allow,
        ];
        let id = if c.id == 0 {
            self.db.execute(
                "INSERT INTO connections (folder_id, name, host, port, dbname, username, password,
                    sslmode, log_dir, ssh_enabled, ssh_host, ssh_port, ssh_user, ssh_auth,
                    ssh_password, ssh_key_path, ssh_key_passphrase, filters, active,
                    include_filters, bridge_enabled, bridge_listen, bridge_port, bridge_allow)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,
                    ?21,?22,?23,?24)",
                p,
            )?;
            self.db.last_insert_rowid()
        } else {
            self.db.execute(
                &format!(
                    "UPDATE connections SET folder_id=?1, name=?2, host=?3, port=?4, dbname=?5,
                        username=?6, password=?7, sslmode=?8, log_dir=?9, ssh_enabled=?10,
                        ssh_host=?11, ssh_port=?12, ssh_user=?13, ssh_auth=?14, ssh_password=?15,
                        ssh_key_path=?16, ssh_key_passphrase=?17, filters=?18, active=?19,
                        include_filters=?20, bridge_enabled=?21, bridge_listen=?22,
                        bridge_port=?23, bridge_allow=?24
                     WHERE id = {}",
                    c.id
                ),
                p,
            )?;
            c.id
        };
        self.db
            .execute("DELETE FROM channels WHERE connection_id = ?1", [id])?;
        for ch in &c.channels {
            self.db.execute(
                "INSERT OR IGNORE INTO channels (connection_id, name) VALUES (?1, ?2)",
                params![id, ch],
            )?;
        }
        Ok(id)
    }

    pub fn delete_connection(&self, id: i64) -> Result<()> {
        self.db
            .execute("DELETE FROM channels WHERE connection_id = ?1", [id])?;
        self.db.execute("DELETE FROM connections WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn save_folder(&self, f: &Folder) -> Result<i64> {
        if f.id == 0 {
            self.db.execute(
                "INSERT INTO folders (parent_id, name) VALUES (?1, ?2)",
                params![f.parent_id, f.name],
            )?;
            Ok(self.db.last_insert_rowid())
        } else {
            self.db.execute(
                "UPDATE folders SET parent_id = ?1, name = ?2 WHERE id = ?3",
                params![f.parent_id, f.name, f.id],
            )?;
            Ok(f.id)
        }
    }

    /// Удаляет папку; её содержимое переносится в родительскую папку.
    pub fn delete_folder(&self, id: i64) -> Result<()> {
        let parent: Option<i64> = self
            .db
            .query_row("SELECT parent_id FROM folders WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .flatten();
        self.db.execute(
            "UPDATE folders SET parent_id = ?1 WHERE parent_id = ?2",
            params![parent, id],
        )?;
        self.db.execute(
            "UPDATE connections SET folder_id = ?1 WHERE folder_id = ?2",
            params![parent, id],
        )?;
        self.db.execute("DELETE FROM folders WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn set_active(&self, id: i64, active: bool) -> Result<()> {
        self.db.execute(
            "UPDATE connections SET active = ?1 WHERE id = ?2",
            params![active, id],
        )?;
        Ok(())
    }

    pub fn setting(&self, key: &str) -> Option<String> {
        self.db
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    fn delete_setting(&self, key: &str) -> Result<()> {
        self.db.execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }

    // ---------- мастер-пароль ----------

    pub fn lock_state(&self) -> Lock {
        if self.setting("crypto_check").is_none() {
            Lock::NoPassword
        } else if self.key.is_some() {
            Lock::Unlocked
        } else {
            Lock::Locked
        }
    }

    fn check_key(&self, key: &Key) -> bool {
        self.setting("crypto_check")
            .is_some_and(|c| crypto::is_encrypted(&c) && key.decrypt(&c).is_ok_and(|v| v == CHECK))
    }

    /// Разблокирует базу мастер-паролем. Возвращает false, если пароль неверный.
    pub fn unlock(&mut self, password: &str, remember: bool) -> Result<bool> {
        let Some(salt) = self.setting("crypto_salt").and_then(|s| crypto::from_hex(&s)) else {
            bail!(t(TKey::NoCryptoParams));
        };
        let key = Key::derive(password, &salt)?;
        if !self.check_key(&key) {
            return Ok(false);
        }
        self.key = Some(key);
        if remember {
            self.remember()?;
        }
        Ok(true)
    }

    /// Пробует ключ, запомненный на этом компьютере (DPAPI).
    pub fn unlock_remembered(&mut self) -> bool {
        #[cfg(windows)]
        {
            let key = self
                .setting("crypto_dpapi")
                .and_then(|s| crypto::from_hex(&s))
                .and_then(|b| crypto::dpapi_unprotect(&b))
                .and_then(|b| Key::from_bytes(&b));
            if let Some(key) = key {
                if self.check_key(&key) {
                    self.key = Some(key);
                    return true;
                }
            }
        }
        false
    }

    pub fn is_remembered(&self) -> bool {
        self.setting("crypto_dpapi").is_some()
    }

    /// Запоминает ключ на этом компьютере (только Windows).
    pub fn remember(&self) -> Result<()> {
        #[cfg(windows)]
        if let Some(key) = &self.key {
            let Some(blob) = crypto::dpapi_protect(key.bytes()) else {
                bail!(t(TKey::DpapiProtectFailed));
            };
            return self.set_setting("crypto_dpapi", &crypto::to_hex(&blob));
        }
        Ok(())
    }

    pub fn forget(&self) -> Result<()> {
        self.delete_setting("crypto_dpapi")
    }

    /// Задаёт, меняет (Some) или снимает (None) мастер-пароль и перешифровывает все пароли.
    /// База должна быть разблокирована.
    pub fn set_master(&mut self, new: Option<&str>, remember: bool) -> Result<()> {
        if self.lock_state() == Lock::Locked {
            bail!(t(TKey::DbLocked));
        }
        let conns = self.connections()?;
        self.db.execute_batch("BEGIN")?;
        let r = (|| -> Result<()> {
            match new {
                Some(pw) => {
                    let salt = crypto::random::<16>();
                    let key = Key::derive(pw, &salt)?;
                    self.set_setting("crypto_salt", &crypto::to_hex(&salt))?;
                    self.set_setting("crypto_check", &key.encrypt(CHECK))?;
                    self.key = Some(key);
                }
                None => {
                    self.delete_setting("crypto_salt")?;
                    self.delete_setting("crypto_check")?;
                    self.key = None;
                }
            }
            self.forget()?;
            for c in &conns {
                self.save_connection(c)?;
            }
            Ok(())
        })();
        match r {
            Ok(()) => self.db.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = self.db.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        if remember && new.is_some() {
            self.remember()?;
        }
        Ok(())
    }

    /// Забытый мастер-пароль: стирает все сохранённые пароли и снимает шифрование.
    pub fn reset_master(&mut self) -> Result<()> {
        self.db.execute(
            "UPDATE connections SET password = '', ssh_password = '', ssh_key_passphrase = ''",
            [],
        )?;
        for k in ["crypto_salt", "crypto_check", "crypto_dpapi"] {
            self.delete_setting(k)?;
        }
        self.key = None;
        Ok(())
    }

    pub fn clear_active(&self) -> Result<()> {
        self.db.execute("UPDATE connections SET active = 0", [])?;
        Ok(())
    }
}
