//! Мост: принимает TCP-подключения из локальной сети и пробрасывает их к серверу БД
//! напрямую или через SSH-туннель подключения. Трафик передаётся как есть, поэтому
//! клиент сам проходит аутентификацию в PostgreSQL и сам договаривается об SSL.

use crate::model::{Cmd, ConnConfig};
use crate::worker::{self, SshHandle};
use anyhow::{anyhow, Context as _, Result};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use crate::i18n::{Key, t, tf};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const BIND_RETRY: Duration = Duration::from_secs(30);

/// Разрешённая подсеть (одиночный адрес — подсеть с полной маской).
#[derive(Clone, Copy, Debug)]
pub struct Net {
    addr: IpAddr,
    prefix: u8,
}

impl Net {
    fn parse(s: &str) -> Option<Self> {
        let (a, p) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let addr: IpAddr = a.trim().parse().ok()?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match p {
            Some(p) => p.trim().parse().ok().filter(|&p| p <= max)?,
            None => max,
        };
        Some(Self { addr, prefix })
    }

    fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip.to_canonical()) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX.checked_shl(32 - self.prefix as u32).unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX.checked_shl(128 - self.prefix as u32).unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// Разбирает список разрешённых адресов; в ошибке — первая неверная запись.
pub fn parse_allow(list: &[String]) -> Result<Vec<Net>, String> {
    list.iter()
        .map(|s| Net::parse(s).ok_or_else(|| tf(Key::BadNet, &[s])))
        .collect()
}

/// Разбивает строку ввода на записи списка разрешённых адресов.
pub fn split_allow(text: &str) -> Vec<String> {
    text.split([',', ';', ' ', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

fn allowed(nets: &[Net], ip: IpAddr) -> bool {
    nets.is_empty() || nets.iter().any(|n| n.contains(ip))
}

/// Работает, пока задачу не остановят; сообщения пишет в служебный лог подключения.
pub async fn run(cfg: ConnConfig, log: UnboundedSender<Cmd>) {
    let say = |text: String| {
        let _ = log.send(Cmd::Log(tf(Key::BridgeMsg, &[&text])));
    };
    let b = &cfg.bridge;
    let nets = match parse_allow(&b.allow) {
        Ok(n) => n,
        Err(e) => return say(tf(Key::BridgeNotStarted, &[&e])),
    };
    let addr = format!("{}:{}", b.listen, b.port);
    let mut attempt = 0;
    let listener = loop {
        attempt += 1;
        match TcpListener::bind((b.listen.as_str(), b.port)).await {
            Ok(l) => break l,
            // После переподключения порт может ещё держать прежний мост: ждём молча.
            Err(_) if attempt < 5 => tokio::time::sleep(Duration::from_secs(1)).await,
            Err(e) => {
                say(tf(
                    Key::BridgeBindFailed,
                    &[&addr, &e.to_string(), &BIND_RETRY.as_secs().to_string()],
                ));
                tokio::time::sleep(BIND_RETRY).await;
            }
        }
    };
    let allow_text = if b.allow.is_empty() {
        t(Key::AllAddresses).to_string()
    } else {
        b.allow.join(", ")
    };
    say(tf(Key::BridgeListening, &[&addr, &allow_text]));

    // Одна SSH-сессия на все подключения через мост, открывается по требованию.
    let ssh: Arc<Mutex<Option<Arc<SshHandle>>>> = Arc::default();
    // При остановке моста JoinSet уничтожается вместе с задачей и обрывает все соединения.
    let mut clients = JoinSet::new();
    loop {
        let (sock, peer) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                say(tf(Key::BridgeAcceptError, &[&e.to_string()]));
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        while clients.try_join_next().is_some() {}
        if !allowed(&nets, peer.ip()) {
            say(tf(Key::PeerRejected, &[&peer.to_string()]));
            continue;
        }
        say(tf(Key::PeerConnected, &[&peer.to_string()]));
        let (cfg, ssh, log) = (cfg.clone(), ssh.clone(), log.clone());
        clients.spawn(async move {
            let text = match serve(sock, peer, &cfg, &ssh).await {
                Ok((up, down)) => tf(
                    Key::PeerDisconnectedBytes,
                    &[&peer.to_string(), &fmt_bytes(up), &fmt_bytes(down)],
                ),
                // Клиент PostgreSQL нередко закрывает сокет сбросом, это обычное завершение.
                Err(e) if is_reset(&e) => tf(Key::PeerDisconnected, &[&peer.to_string()]),
                Err(e) => tf(
                    Key::PeerDisconnectedErr,
                    &[&peer.to_string(), &format!("{e:#}")],
                ),
            };
            let _ = log.send(Cmd::Log(tf(Key::BridgeMsgText, &[&text])));
        });
    }
}

async fn serve(
    mut client: TcpStream,
    peer: SocketAddr,
    cfg: &ConnConfig,
    ssh: &Mutex<Option<Arc<SshHandle>>>,
) -> Result<(u64, u64)> {
    let _ = client.set_nodelay(true);
    let r = if cfg.ssh.enabled {
        let mut upstream = ssh_stream(cfg, ssh, peer).await?;
        tokio::io::copy_bidirectional(&mut client, &mut upstream).await
    } else {
        let mut upstream =
            tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((cfg.host.as_str(), cfg.port)))
                .await
                .map_err(|_| anyhow!(tf(Key::ConnectTimeoutAddr, &[&cfg.host, &cfg.port.to_string()])))?
                .with_context(|| tf(Key::ConnectingTo, &[&cfg.host, &cfg.port.to_string()]))?;
        let _ = upstream.set_nodelay(true);
        tokio::io::copy_bidirectional(&mut client, &mut upstream).await
    };
    Ok(r?)
}

/// Открывает канал до БД в общей SSH-сессии, при необходимости переподключаясь к SSH.
async fn ssh_stream(
    cfg: &ConnConfig,
    ssh: &Mutex<Option<Arc<SshHandle>>>,
    peer: SocketAddr,
) -> Result<worker::SshStream> {
    let origin = peer.ip().to_string();
    let mut guard = ssh.lock().await;
    if let Some(h) = guard.as_ref().filter(|h| !h.is_closed()) {
        if let Ok(s) = worker::ssh_forward(h, cfg, &origin, peer.port()).await {
            return Ok(s);
        }
    }
    // Сессии нет или она перестала работать: открываем новую.
    *guard = None;
    let h = Arc::new(
        tokio::time::timeout(CONNECT_TIMEOUT, worker::ssh_connect(cfg))
            .await
            .map_err(|_| anyhow!(t(Key::SshConnectTimeout)))??,
    );
    let s = worker::ssh_forward(&h, cfg, &origin, peer.port()).await?;
    *guard = Some(h);
    Ok(s)
}

fn is_reset(e: &anyhow::Error) -> bool {
    use std::io::ErrorKind as K;
    e.downcast_ref::<std::io::Error>().is_some_and(|e| {
        matches!(e.kind(), K::ConnectionReset | K::ConnectionAborted | K::BrokenPipe)
    })
}

fn fmt_bytes(n: u64) -> String {
    if n < 1024 {
        tf(Key::UnitB, &[&n.to_string()])
    } else if n < 1024 * 1024 {
        tf(Key::UnitKb, &[&format!("{:.1}", n as f64 / 1024.0)])
    } else {
        tf(Key::UnitMb, &[&format!("{:.1}", n as f64 / (1024.0 * 1024.0))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nets() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let n = parse_allow(&split_allow("192.168.1.0/24, 10.0.0.5 ::1")).unwrap();
        assert!(allowed(&n, ip("192.168.1.77")));
        assert!(!allowed(&n, ip("192.168.2.1")));
        assert!(allowed(&n, ip("10.0.0.5")));
        assert!(!allowed(&n, ip("10.0.0.6")));
        assert!(allowed(&n, ip("::ffff:192.168.1.3")));
        assert!(allowed(&n, ip("::1")));
        assert!(allowed(&[], ip("8.8.8.8")));
        assert!(allowed(&parse_allow(&["0.0.0.0/0".into()]).unwrap(), ip("8.8.8.8")));
        assert!(parse_allow(&["192.168.1.0/33".into()]).is_err());
        assert!(parse_allow(&["хост".into()]).is_err());
    }
}
