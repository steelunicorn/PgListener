use chrono::NaiveDateTime;
use std::sync::Arc;
use crate::i18n::{Key, t};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SslMode {
    Disable,
    Prefer,
    Require,
}

impl SslMode {
    pub const ALL: [SslMode; 3] = [SslMode::Disable, SslMode::Prefer, SslMode::Require];

    pub fn as_str(self) -> &'static str {
        match self {
            SslMode::Disable => "disable",
            SslMode::Prefer => "prefer",
            SslMode::Require => "require",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "disable" => SslMode::Disable,
            "require" => SslMode::Require,
            _ => SslMode::Prefer,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SshAuth {
    Password,
    Key,
}

impl SshAuth {
    pub fn as_str(self) -> &'static str {
        match self {
            SshAuth::Password => "password",
            SshAuth::Key => "key",
        }
    }

    pub fn parse(s: &str) -> Self {
        if s == "key" {
            SshAuth::Key
        } else {
            SshAuth::Password
        }
    }
}

#[derive(Clone, Debug)]
pub struct SshConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
    pub password: String,
    pub key_path: String,
    pub key_passphrase: String,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: String::new(),
            port: 22,
            user: String::new(),
            auth: SshAuth::Password,
            password: String::new(),
            key_path: String::new(),
            key_passphrase: String::new(),
        }
    }
}

/// Мост: приём TCP-подключений из локальной сети и проброс их к БД (через SSH, если он включён).
#[derive(Clone, Debug)]
pub struct BridgeConfig {
    pub enabled: bool,
    pub listen: String,
    pub port: u16,
    /// Разрешённые адреса и подсети (`192.168.1.10`, `192.168.1.0/24`); пустой список — все.
    pub allow: Vec<String>,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: "0.0.0.0".into(),
            port: 15432,
            allow: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConnConfig {
    pub id: i64,
    pub folder_id: Option<i64>,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub dbname: String,
    pub user: String,
    pub password: String,
    pub sslmode: SslMode,
    /// Подкаталог внутри logs/. Пустая строка означает «имя подключения».
    pub log_dir: String,
    pub ssh: SshConfig,
    /// Подстроки-исключения: строки с любой из них скрываются.
    pub filters: Vec<String>,
    /// Подстроки-включения: если список не пуст, показываются только строки с любой из них.
    pub includes: Vec<String>,
    pub bridge: BridgeConfig,
    pub channels: Vec<String>,
    pub active: bool,
}

impl Default for ConnConfig {
    fn default() -> Self {
        Self {
            id: 0,
            folder_id: None,
            name: t(Key::NewConnection).into(),
            host: "localhost".into(),
            port: 5432,
            dbname: "postgres".into(),
            user: "postgres".into(),
            password: String::new(),
            sslmode: SslMode::Prefer,
            log_dir: String::new(),
            ssh: SshConfig::default(),
            filters: Vec::new(),
            includes: Vec::new(),
            bridge: BridgeConfig::default(),
            channels: Vec::new(),
            active: false,
        }
    }
}

impl ConnConfig {
    pub fn effective_log_dir(&self) -> String {
        let d = self.log_dir.trim();
        if d.is_empty() {
            self.name.clone()
        } else {
            d.to_string()
        }
    }
}

#[derive(Clone, Debug)]
pub struct Folder {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
}

/// Одна строка лога. Пустой `channel` означает служебное сообщение приложения.
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub time: NaiveDateTime,
    pub channel: Arc<str>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConnStatus {
    Disconnected,
    Connecting,
    Connected,
    /// Ждём перед повторной попыткой; строка содержит причину.
    Reconnecting(String),
}

pub enum Event {
    /// (id подключения, номер сессии воркера, состояние)
    Status(i64, u64, ConnStatus),
    Entry(i64, LogEntry),
    /// История текущих суток для канала (заменяет то, что уже показано по этому каналу).
    History(i64, Arc<str>, Vec<LogEntry>),
    TestResult(Result<String, String>),
}

pub enum Cmd {
    Listen(String),
    /// Служебное сообщение в лог подключения (например, от моста).
    Log(String),
    Unlisten(String),
    Stop,
}
