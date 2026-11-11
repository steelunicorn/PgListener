//! Запись логов каналов в файлы с посуточной ротацией и чтение записей за текущие сутки.
//!
//! Текущий файл канала называется `<канал>.log`. Когда наступают новые сутки (или при
//! старте обнаруживается, что файл последний раз писался в прошлые сутки), он переименовывается
//! в `<канал>.<ГГГГ-ММ-ДД>.log`, и запись продолжается в новый `<канал>.log`.

use crate::model::LogEntry;
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Имя файла для служебных сообщений приложения.
pub const SYSTEM_LOG: &str = "_pglistener";
const TIME_FMT: &str = "%Y-%m-%d %H:%M:%S%.3f";
const TIME_LEN: usize = 23;

pub fn file_stem(channel: &str) -> String {
    if channel.is_empty() {
        return SYSTEM_LOG.to_string();
    }
    channel
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect()
}

fn current_path(dir: &Path, channel: &str) -> PathBuf {
    dir.join(format!("{}.log", file_stem(channel)))
}

fn modified_date(path: &Path) -> Option<NaiveDate> {
    let m = fs::metadata(path).ok()?.modified().ok()?;
    Some(DateTime::<Local>::from(m).date_naive())
}

/// Переименовывает `<канал>.log` в `<канал>.<дата>.log`, если он относится к другим суткам.
fn rotate_if_stale(dir: &Path, channel: &str, today: NaiveDate) {
    let path = current_path(dir, channel);
    let Some(date) = modified_date(&path) else {
        return;
    };
    if date == today {
        return;
    }
    let stem = file_stem(channel);
    let mut target = dir.join(format!("{stem}.{date}.log"));
    let mut n = 1;
    while target.exists() {
        target = dir.join(format!("{stem}.{date}.{n}.log"));
        n += 1;
    }
    let _ = fs::rename(&path, &target);
}

struct OpenFile {
    date: NaiveDate,
    out: BufWriter<File>,
}

pub struct LogWriter {
    dir: PathBuf,
    files: HashMap<String, OpenFile>,
}

impl LogWriter {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            files: HashMap::new(),
        }
    }

    pub fn write(&mut self, e: &LogEntry) {
        if let Err(err) = self.try_write(e) {
            eprintln!("log write error: {err}");
        }
    }

    fn try_write(&mut self, e: &LogEntry) -> std::io::Result<()> {
        let date = e.time.date();
        let key: &str = &e.channel;
        if self.files.get(key).is_some_and(|f| f.date != date) {
            if let Some(mut f) = self.files.remove(key) {
                f.out.flush()?;
            }
        }
        if !self.files.contains_key(key) {
            fs::create_dir_all(&self.dir)?;
            rotate_if_stale(&self.dir, key, date);
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(current_path(&self.dir, key))?;
            self.files.insert(
                key.to_string(),
                OpenFile {
                    date,
                    out: BufWriter::new(file),
                },
            );
        }
        let f = self.files.get_mut(key).unwrap();
        writeln!(f.out, "{}: {}", e.time.format(TIME_FMT), e.text)?;
        f.out.flush()
    }

    /// Закрывает файл канала (после UNLISTEN).
    pub fn close(&mut self, channel: &str) {
        self.files.remove(channel);
    }
}

fn parse_time(line: &str) -> Option<NaiveDateTime> {
    let head = line.get(..TIME_LEN)?;
    if line.get(TIME_LEN..TIME_LEN + 2) != Some(": ") {
        return None;
    }
    NaiveDateTime::parse_from_str(head, TIME_FMT).ok()
}

/// Читает записи за `today` из текущего файла канала. Многострочные сообщения склеиваются.
pub fn read_today(dir: &Path, channel: &str, today: NaiveDate) -> Vec<LogEntry> {
    let path = current_path(dir, channel);
    if modified_date(&path) != Some(today) {
        return Vec::new();
    }
    let Ok(bytes) = fs::read(&path) else {
        return Vec::new();
    };
    let content = String::from_utf8_lossy(&bytes);
    let channel: Arc<str> = Arc::from(channel);
    let mut out: Vec<LogEntry> = Vec::new();
    // Строки-продолжения, относящиеся к записи за прошлые сутки, нужно пропустить.
    let mut skipping = true;
    for line in content.lines() {
        match parse_time(line) {
            Some(t) => {
                skipping = t.date() != today;
                if !skipping {
                    out.push(LogEntry {
                        time: t,
                        channel: channel.clone(),
                        text: line[TIME_LEN + 2..].to_string(),
                    });
                }
            }
            None if !skipping => {
                if let Some(last) = out.last_mut() {
                    last.text.push('\n');
                    last.text.push_str(line);
                }
            }
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_and_read_back() {
        let dir = std::env::temp_dir().join(format!("pgl-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let now = Local::now().naive_local();
        let mut w = LogWriter::new(dir.clone());
        let ch: Arc<str> = Arc::from("my:chan");
        w.write(&LogEntry {
            time: now,
            channel: ch.clone(),
            text: "first".into(),
        });
        w.write(&LogEntry {
            time: now,
            channel: ch.clone(),
            text: "multi\nline".into(),
        });
        let got = read_today(&dir, "my:chan", now.date());
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].text, "multi\nline");
        assert!(dir.join("my_chan.log").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
