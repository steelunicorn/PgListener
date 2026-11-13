//! Фоновая часть: SSH-туннель, подключение к PostgreSQL, LISTEN/UNLISTEN, переподключение
//! и запись логов. Каждое активное подключение обслуживается отдельной задачей tokio.

use crate::logfile::{self, LogWriter};
use crate::model::*;
use anyhow::{anyhow, bail, Context as _, Result};
use chrono::Local;
use futures_util::StreamExt;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres::AsyncMessage;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const PING_INTERVAL: Duration = Duration::from_secs(30);
const PING_TIMEOUT: Duration = Duration::from_secs(15);
const BACKOFF: [u64; 6] = [1, 2, 5, 10, 20, 30];

/// Канал событий в GUI; каждое событие будит перерисовку окна.
#[derive(Clone)]
pub struct EventSink {
    tx: mpsc::Sender<Event>,
    ctx: egui::Context,
}

impl EventSink {
    pub fn new(tx: mpsc::Sender<Event>, ctx: egui::Context) -> Self {
        Self { tx, ctx }
    }

    pub fn send(&self, e: Event) {
        // Новые строки лога показываем не чаще 10 раз в секунду: при потоке сообщений
        // перерисовка на каждое из них заметно грузит процессор.
        let delay = match e {
            Event::Entry(..) => Duration::from_millis(100),
            _ => Duration::ZERO,
        };
        let _ = self.tx.send(e);
        self.ctx.request_repaint_after(delay);
    }
}

pub fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Останавливает задачу tokio, когда владелец уходит из области видимости.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Worker {
    session: u64,
    cfg: ConnConfig,
    channels: BTreeSet<String>,
    log: LogWriter,
    log_dir: PathBuf,
    events: EventSink,
}

enum Outcome {
    Stop,
}

impl Worker {
    fn status(&self, s: ConnStatus) {
        self.events.send(Event::Status(self.cfg.id, self.session, s));
    }

    /// Служебное сообщение: только в окно, в файлы логов не пишется.
    fn system(&mut self, text: impl Into<String>) {
        let e = LogEntry {
            time: Local::now().naive_local(),
            channel: Arc::from(""),
            text: text.into(),
        };
        self.events.send(Event::Entry(self.cfg.id, e));
    }

    fn entry(&mut self, e: LogEntry) {
        self.log.write(&e);
        self.events.send(Event::Entry(self.cfg.id, e));
    }

    fn send_history(&self, channel: &str) {
        let today = Local::now().date_naive();
        let entries = logfile::read_today(&self.log_dir, channel, today);
        self.events
            .send(Event::History(self.cfg.id, Arc::from(channel), entries));
    }

    /// Обрабатывает команду, пока соединения нет. Возвращает true, если нужно остановиться.
    fn apply_offline(&mut self, cmd: Option<Cmd>) -> bool {
        match cmd {
            Some(Cmd::Listen(ch)) => {
                if self.channels.insert(ch.clone()) {
                    self.send_history(&ch);
                }
                false
            }
            Some(Cmd::Unlisten(ch)) => {
                self.channels.remove(&ch);
                self.log.close(&ch);
                false
            }
            Some(Cmd::Log(text)) => {
                self.system(text);
                false
            }
            Some(Cmd::Stop) | None => true,
        }
    }

    async fn run(mut self, mut cmds: UnboundedReceiver<Cmd>) {
        for ch in self.channels.clone() {
            self.send_history(&ch);
        }
        let mut attempt = 0usize;
        loop {
            self.status(ConnStatus::Connecting);
            let started = tokio::time::Instant::now();
            let err = match self.session(&mut cmds).await {
                Ok(Outcome::Stop) => break,
                Err(e) => e,
            };
            // Если соединение успело поработать, начинаем отсчёт задержек заново.
            if started.elapsed() > Duration::from_secs(60) {
                attempt = 0;
            }
            let delay = BACKOFF[attempt.min(BACKOFF.len() - 1)];
            attempt += 1;
            let reason = format!("{err:#}");
            self.system(format!(
                "Ошибка: {reason}. Повторное подключение через {delay} с"
            ));
            self.status(ConnStatus::Reconnecting(reason));
            let sleep = tokio::time::sleep(Duration::from_secs(delay));
            tokio::pin!(sleep);
            let stop = loop {
                tokio::select! {
                    _ = &mut sleep => break false,
                    cmd = cmds.recv() => if self.apply_offline(cmd) { break true },
                }
            };
            if stop {
                break;
            }
        }
        self.system("Отключено");
        self.status(ConnStatus::Disconnected);
    }

    async fn session(&mut self, cmds: &mut UnboundedReceiver<Cmd>) -> Result<Outcome> {
        let cfg = self.cfg.clone();
        if cfg.ssh.enabled {
            let (handle, stream) = tokio::time::timeout(CONNECT_TIMEOUT, ssh_tunnel(&cfg))
                .await
                .map_err(|_| anyhow!("таймаут подключения к SSH-серверу"))??;
            let r = self.pg_session(stream, cmds).await;
            let _ = handle
                .disconnect(russh::Disconnect::ByApplication, "", "")
                .await;
            r
        } else {
            let stream = tokio::time::timeout(
                CONNECT_TIMEOUT,
                tokio::net::TcpStream::connect((cfg.host.as_str(), cfg.port)),
            )
            .await
            .map_err(|_| anyhow!("таймаут подключения к {}:{}", cfg.host, cfg.port))?
            .with_context(|| format!("подключение к {}:{}", cfg.host, cfg.port))?;
            let _ = stream.set_nodelay(true);
            self.pg_session(stream, cmds).await
        }
    }

    async fn pg_session<S>(
        &mut self,
        stream: S,
        cmds: &mut UnboundedReceiver<Cmd>,
    ) -> Result<Outcome>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (client, mut notifications, conn_task) = pg_connect(&self.cfg, stream).await?;
        // Гарантирует остановку фоновой задачи соединения при выходе из функции.
        let _guard = AbortOnDrop(conn_task);

        for ch in &self.channels {
            client
                .batch_execute(&format!("LISTEN {}", quote_ident(ch)))
                .await
                .with_context(|| format!("LISTEN {ch}"))?;
        }
        let list: Vec<&str> = self.channels.iter().map(String::as_str).collect();
        self.system(format!(
            "Подключено к {}:{}/{}{}. Каналы: {}",
            self.cfg.host,
            self.cfg.port,
            self.cfg.dbname,
            if self.cfg.ssh.enabled {
                format!(" через SSH {}", self.cfg.ssh.host)
            } else {
                String::new()
            },
            if list.is_empty() {
                "(нет)".to_string()
            } else {
                list.join(", ")
            }
        ));
        self.status(ConnStatus::Connected);

        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.reset();
        loop {
            tokio::select! {
                n = notifications.recv() => match n {
                    Some(Ok(n)) => self.entry(LogEntry {
                        time: Local::now().naive_local(),
                        channel: Arc::from(n.channel()),
                        text: n.payload().to_string(),
                    }),
                    Some(Err(e)) => return Err(e.into()),
                    None => bail!("соединение закрыто сервером"),
                },
                cmd = cmds.recv() => match cmd {
                    Some(Cmd::Listen(ch)) => {
                        if self.channels.insert(ch.clone()) {
                            self.send_history(&ch);
                            client.batch_execute(&format!("LISTEN {}", quote_ident(&ch))).await?;
                            self.system(format!("Добавлен канал {ch}"));
                        }
                    }
                    Some(Cmd::Unlisten(ch)) => {
                        if self.channels.remove(&ch) {
                            client.batch_execute(&format!("UNLISTEN {}", quote_ident(&ch))).await?;
                            self.log.close(&ch);
                            self.system(format!("Удалён канал {ch}"));
                        }
                    }
                    Some(Cmd::Log(text)) => self.system(text),
                    Some(Cmd::Stop) | None => return Ok(Outcome::Stop),
                },
                _ = ping.tick() => {
                    tokio::time::timeout(PING_TIMEOUT, client.simple_query("SELECT 1"))
                        .await
                        .map_err(|_| anyhow!("сервер не отвечает"))??;
                }
            }
        }
    }
}

type Notifications = UnboundedReceiver<Result<tokio_postgres::Notification, tokio_postgres::Error>>;

async fn pg_connect<S>(
    cfg: &ConnConfig,
    stream: S,
) -> Result<(tokio_postgres::Client, Notifications, tokio::task::JoinHandle<()>)>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut pg = tokio_postgres::Config::new();
    pg.host(&cfg.host)
        .port(cfg.port)
        .user(&cfg.user)
        .dbname(&cfg.dbname)
        .application_name("pglistener")
        .connect_timeout(CONNECT_TIMEOUT)
        .ssl_mode(match cfg.sslmode {
            SslMode::Disable => tokio_postgres::config::SslMode::Disable,
            SslMode::Prefer => tokio_postgres::config::SslMode::Prefer,
            SslMode::Require => tokio_postgres::config::SslMode::Require,
        });
    if !cfg.password.is_empty() {
        pg.password(&cfg.password);
    }
    // Как и libpq в режимах prefer/require, сертификат сервера не проверяем.
    let tls = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()?;
    let mut make = postgres_native_tls::MakeTlsConnector::new(tls);
    let connector = MakeTlsConnect::<S>::make_tls_connect(&mut make, &cfg.host)?;
    let (client, mut connection) = tokio::time::timeout(CONNECT_TIMEOUT, pg.connect_raw(stream, connector))
        .await
        .map_err(|_| anyhow!("таймаут подключения к PostgreSQL"))??;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        let mut messages = futures_util::stream::poll_fn(move |cx| connection.poll_message(cx));
        while let Some(m) = messages.next().await {
            match m {
                Ok(AsyncMessage::Notification(n)) => {
                    if tx.send(Ok(n)).is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            }
        }
    });
    Ok((client, rx, task))
}

pub struct SshClient;

impl russh::client::Handler for SshClient {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

fn resolve_path(p: &str) -> PathBuf {
    let path = PathBuf::from(p);
    if path.is_absolute() {
        path
    } else {
        crate::app_dir().join(path)
    }
}

pub type SshHandle = russh::client::Handle<SshClient>;
pub type SshStream = russh::ChannelStream<russh::client::Msg>;

async fn ssh_tunnel(cfg: &ConnConfig) -> Result<(SshHandle, SshStream)> {
    let handle = ssh_connect(cfg).await?;
    let stream = ssh_forward(&handle, cfg, "127.0.0.1", 0).await?;
    Ok((handle, stream))
}

/// Открывает в SSH-сессии канал до сервера БД.
pub async fn ssh_forward(
    handle: &SshHandle,
    cfg: &ConnConfig,
    origin: &str,
    origin_port: u16,
) -> Result<SshStream> {
    let channel = handle
        .channel_open_direct_tcpip(cfg.host.clone(), cfg.port as u32, origin, origin_port as u32)
        .await
        .with_context(|| format!("SSH-туннель до {}:{}", cfg.host, cfg.port))?;
    Ok(channel.into_stream())
}

/// Подключается к SSH-серверу подключения и проходит аутентификацию.
pub async fn ssh_connect(cfg: &ConnConfig) -> Result<SshHandle> {
    let s = &cfg.ssh;
    let config = Arc::new(russh::client::Config {
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        nodelay: true,
        ..Default::default()
    });
    let mut handle = russh::client::connect(config, (s.host.as_str(), s.port), SshClient)
        .await
        .with_context(|| format!("подключение к SSH {}:{}", s.host, s.port))?;

    let ok = match s.auth {
        SshAuth::Password => {
            let r = handle.authenticate_password(&s.user, &s.password).await?;
            r.success() || keyboard_interactive(&mut handle, &s.user, &s.password).await?
        }
        SshAuth::Key => {
            let pass = (!s.key_passphrase.is_empty()).then_some(s.key_passphrase.as_str());
            let key = russh::keys::load_secret_key(resolve_path(&s.key_path), pass)
                .with_context(|| format!("чтение ключа {}", s.key_path))?;
            let hash = handle.best_supported_rsa_hash().await?.flatten();
            handle
                .authenticate_publickey(
                    &s.user,
                    russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await?
                .success()
        }
    };
    if !ok {
        bail!("SSH: аутентификация не удалась");
    }
    Ok(handle)
}

/// Некоторые серверы принимают пароль только через keyboard-interactive.
async fn keyboard_interactive(
    handle: &mut russh::client::Handle<SshClient>,
    user: &str,
    password: &str,
) -> Result<bool> {
    use russh::client::KeyboardInteractiveAuthResponse as R;
    let mut r = handle
        .authenticate_keyboard_interactive_start(user, None)
        .await?;
    for _ in 0..5 {
        match r {
            R::Success => return Ok(true),
            R::Failure { .. } => return Ok(false),
            R::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| password.to_string()).collect();
                r = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await?;
            }
        }
    }
    Ok(false)
}

/// Запускает задачу прослушивания подключения и возвращает канал команд для неё.
pub fn spawn(
    rt: &tokio::runtime::Handle,
    session: u64,
    cfg: ConnConfig,
    log_dir: PathBuf,
    events: EventSink,
) -> UnboundedSender<Cmd> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let bridge = cfg
        .bridge
        .enabled
        .then(|| AbortOnDrop(rt.spawn(crate::bridge::run(cfg.clone(), tx.clone()))));
    let worker = Worker {
        session,
        channels: cfg.channels.iter().cloned().collect(),
        log: LogWriter::new(log_dir.clone()),
        log_dir,
        cfg,
        events,
    };
    rt.spawn(async move {
        worker.run(rx).await;
        // Мост работает, пока подключение активно.
        drop(bridge);
    });
    tx
}

/// Проверка подключения из диалога настроек: подключается и возвращает версию сервера.
pub fn test(rt: &tokio::runtime::Handle, cfg: ConnConfig, events: EventSink) {
    rt.spawn(async move {
        let r = tokio::time::timeout(Duration::from_secs(30), test_inner(cfg))
            .await
            .unwrap_or_else(|_| Err(anyhow!("таймаут")));
        events.send(Event::TestResult(r.map_err(|e| format!("{e:#}"))));
    });
}

async fn test_inner(cfg: ConnConfig) -> Result<String> {
    async fn query<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
        cfg: &ConnConfig,
        stream: S,
    ) -> Result<String> {
        let (client, _n, task) = pg_connect(cfg, stream).await?;
        let row = client.query_one("SELECT version()", &[]).await;
        task.abort();
        Ok(row?.get(0))
    }
    if cfg.ssh.enabled {
        let (handle, stream) = ssh_tunnel(&cfg).await?;
        let r = query(&cfg, stream).await;
        let _ = handle
            .disconnect(russh::Disconnect::ByApplication, "", "")
            .await;
        r
    } else {
        let stream = tokio::net::TcpStream::connect((cfg.host.as_str(), cfg.port))
            .await
            .with_context(|| format!("подключение к {}:{}", cfg.host, cfg.port))?;
        query(&cfg, stream).await
    }
}

