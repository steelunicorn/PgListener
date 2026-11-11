use crate::model::*;
use crate::storage::{Lock, Storage};
use crate::tray::{Tray, TrayCmd};
use crate::worker::{self, EventSink};
use chrono::{Local, NaiveDate};
use egui::text::LayoutJob;
use egui::{Color32, FontId, RichText, TextFormat};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;

const GREEN: Color32 = Color32::from_rgb(60, 180, 75);
const ORANGE: Color32 = Color32::from_rgb(230, 150, 30);
const GRAY: Color32 = Color32::from_rgb(140, 140, 140);

struct Live {
    session: u64,
    tx: UnboundedSender<Cmd>,
    status: ConnStatus,
}

struct Row {
    entry: LogEntry,
    lower: String,
    /// Текст в одну строку (переводы строк заменены на ¶).
    line: String,
}

impl Row {
    fn new(entry: LogEntry) -> Self {
        let line = entry.text.replace("\r\n", "\n").replace('\n', " ¶ ");
        Self {
            lower: entry.text.to_lowercase(),
            line,
            entry,
        }
    }
}

#[derive(Default)]
struct View {
    rows: Vec<Row>,
    /// Растёт, когда строки меняются не только добавлением в конец.
    generation: u64,
    filter_version: u64,
    /// None — вкладка «Все».
    tab: Option<Arc<str>>,
    filter_input: String,
    include_input: String,
    channel_input: String,
    cache_key: (Option<Arc<str>>, u64, u64),
    cache_seen: usize,
    cache: Vec<usize>,
    search: Search,
    /// Сколько строк было показано в прошлом кадре.
    drawn: usize,
}

/// Поиск по показанным строкам.
#[derive(Default)]
struct Search {
    query: String,
    /// Позиции совпавших строк в списке показанных.
    matches: Vec<usize>,
    /// Текущее совпадение (индекс в `matches`).
    current: Option<usize>,
    key: (String, Option<Arc<str>>, u64, u64, usize),
    /// Прокрутить к этой позиции на следующем кадре.
    scroll_to: Option<usize>,
}

impl Search {
    /// Пересчитывает совпадения, если изменился запрос или набор показанных строк.
    fn update(&mut self, rows: &[Row], visible: &[usize], key: (Option<Arc<str>>, u64, u64)) {
        if self.query == self.key.0
            && (&key.0, key.1, key.2) == (&self.key.1, self.key.2, self.key.3)
            && visible.len() >= self.key.4
        {
            // Строки только добавились в конец: проверяем лишь новые, позиции старых не меняются.
            let seen = self.key.4;
            if visible.len() > seen && !self.query.is_empty() {
                let q = self.query.to_lowercase();
                self.matches.extend(
                    (seen..visible.len()).filter(|&pos| rows[visible[pos]].lower.contains(q.as_str())),
                );
                if self.current.is_none() {
                    self.current = self.matches.len().checked_sub(1);
                }
            }
            self.key.4 = visible.len();
            return;
        }
        let key = (self.query.clone(), key.0, key.1, key.2, visible.len());
        let query_changed = key.0 != self.key.0;
        let current_row = self
            .current
            .and_then(|c| self.matches.get(c))
            .and_then(|&pos| visible.get(pos).copied());
        self.key = key;
        self.matches.clear();
        let q = self.query.to_lowercase();
        if !q.is_empty() {
            self.matches = visible
                .iter()
                .enumerate()
                .filter(|(_, &i)| rows[i].lower.contains(q.as_str()))
                .map(|(pos, _)| pos)
                .collect();
        }
        if query_changed {
            // Новый запрос: встаём на самое свежее совпадение.
            self.current = self.matches.len().checked_sub(1);
            self.scroll_to = self.current.map(|c| self.matches[c]);
        } else {
            // Добавились строки: остаёмся на той же строке.
            self.current = current_row
                .and_then(|row| self.matches.iter().position(|&pos| visible[pos] == row))
                .or(self.matches.len().checked_sub(1));
        }
    }

    fn step(&mut self, forward: bool) {
        let n = self.matches.len();
        if n == 0 {
            return;
        }
        let c = match self.current {
            None => n - 1,
            Some(c) if forward => (c + 1) % n,
            Some(c) => (c + n - 1) % n,
        };
        self.current = Some(c);
        self.scroll_to = Some(self.matches[c]);
    }
}

impl View {
    fn push(&mut self, e: LogEntry) {
        // Сообщения приходят по порядку, но история может быть «новее» живых строк.
        if self.rows.last().is_some_and(|r| r.entry.time > e.time) {
            self.generation += 1;
            let pos = self.rows.partition_point(|r| r.entry.time <= e.time);
            self.rows.insert(pos, Row::new(e));
        } else {
            self.rows.push(Row::new(e));
        }
    }

    fn replace_channel(&mut self, channel: &str, entries: Vec<LogEntry>) {
        self.rows.retain(|r| &*r.entry.channel != channel);
        self.rows.extend(entries.into_iter().map(Row::new));
        self.rows.sort_by_key(|r| r.entry.time);
        self.generation += 1;
    }

    fn retain_day(&mut self, day: NaiveDate) {
        self.rows.retain(|r| r.entry.time.date() == day);
        self.generation += 1;
    }

    fn visible(&mut self, filters: &[String], includes: &[String]) -> &[usize] {
        let key = (self.tab.clone(), self.generation, self.filter_version);
        if key != self.cache_key {
            self.cache_key = key;
            self.cache.clear();
            self.cache_seen = 0;
        }
        let lower: Vec<String> = filters.iter().map(|f| f.to_lowercase()).collect();
        let include: Vec<String> = includes.iter().map(|f| f.to_lowercase()).collect();
        for (i, r) in self.rows.iter().enumerate().skip(self.cache_seen) {
            if let Some(t) = &self.tab {
                if *r.entry.channel != **t {
                    continue;
                }
            }
            if lower.iter().any(|f| r.lower.contains(f.as_str())) {
                continue;
            }
            if !include.is_empty() && !include.iter().any(|f| r.lower.contains(f.as_str())) {
                continue;
            }
            self.cache.push(i);
        }
        self.cache_seen = self.rows.len();
        &self.cache
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Folder(i64),
    Conn(i64),
}

struct ConnDialog {
    cfg: ConnConfig,
    new_channel: String,
    test: Option<Result<String, String>>,
    testing: bool,
    /// Список разрешённых адресов моста в том виде, как его ввели.
    allow_text: String,
    error: Option<String>,
}

impl ConnDialog {
    fn new(cfg: ConnConfig) -> Box<Self> {
        Box::new(Self {
            allow_text: cfg.bridge.allow.join(", "),
            cfg,
            new_channel: String::new(),
            test: None,
            testing: false,
            error: None,
        })
    }
}

#[derive(Default)]
struct MasterDialog {
    new: String,
    repeat: String,
    remember: bool,
    error: Option<String>,
}

enum Dialog {
    Conn(Box<ConnDialog>),
    Folder(Folder),
    Delete(Target),
    Master(MasterDialog),
}

/// Окно ввода мастер-пароля при запуске.
#[derive(Default)]
struct UnlockPrompt {
    password: String,
    remember: bool,
    error: bool,
    confirm_reset: bool,
}

enum Action {
    Select(i64),
    Connect(i64),
    Disconnect(i64),
    Edit(i64),
    NewConn(Option<i64>),
    NewFolder(Option<i64>),
    RenameFolder(i64),
    Delete(Target),
    Move(Target, Option<i64>),
    Duplicate(i64),
    Master,
}

pub struct App {
    rt: tokio::runtime::Runtime,
    storage: Storage,
    events: mpsc::Receiver<Event>,
    sink: EventSink,
    folders: Vec<Folder>,
    conns: Vec<ConnConfig>,
    live: HashMap<i64, Live>,
    views: HashMap<i64, View>,
    next_session: u64,
    selected: Option<i64>,
    dialog: Option<Dialog>,
    restore: Vec<i64>,
    error: Option<String>,
    today: NaiveDate,
    unlock: Option<UnlockPrompt>,
    tray: Option<Tray>,
    minimize_to_tray: bool,
    hidden: bool,
    /// Выделение, о котором egui сообщил в конце прошлого кадра.
    settled: Arc<Mutex<Option<String>>>,
    /// Выделенный в логе текст (в нижнем регистре), его вхождения подсвечиваются.
    marked: String,
}

/// Применяет прокрутку колесом мыши за один кадр. egui растягивает каждый щелчок колеса
/// на десяток кадров плавной анимации, и каждый из них перерисовывает весь лог.
/// Мелкие шаги в пикселях egui применяет сразу, поэтому дробим на них щелчки колеса.
struct InstantWheel;

impl egui::Plugin for InstantWheel {
    fn debug_name(&self) -> &'static str {
        "InstantWheel"
    }

    fn input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        const STEP: f32 = 7.0; // egui считает плавными шаги короче 8 точек
        let speed = ctx.options(|o| o.input_options.line_scroll_speed);
        let mut events = Vec::with_capacity(input.events.len());
        for e in input.events.drain(..) {
            match e {
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta,
                    phase,
                    source,
                    modifiers,
                } => {
                    let delta = delta * speed;
                    let n = (delta.length() / STEP).ceil().max(1.0) as usize;
                    for _ in 0..n.min(200) {
                        events.push(egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: delta / n as f32,
                            phase,
                            source,
                            modifiers,
                        });
                    }
                }
                e => events.push(e),
            }
        }
        input.events = events;
    }
}

/// Ловит текст, выделенный мышью: egui отдаёт его только в выходных командах кадра.
struct SelectionWatch(Arc<Mutex<Option<String>>>);

impl egui::Plugin for SelectionWatch {
    fn debug_name(&self) -> &'static str {
        "SelectionWatch"
    }

    fn output_hook(&mut self, _ctx: &egui::Context, output: &mut egui::FullOutput) {
        for cmd in &output.platform_output.commands {
            if let egui::OutputCommand::TextSelectionSettled(text) = cmd {
                *self.0.lock().unwrap() = Some(text.clone());
            }
        }
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, rt: tokio::runtime::Runtime, storage: Storage) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            rt,
            storage,
            events: rx,
            sink: EventSink::new(tx, cc.egui_ctx.clone()),
            folders: Vec::new(),
            conns: Vec::new(),
            live: HashMap::new(),
            views: HashMap::new(),
            next_session: 1,
            selected: None,
            dialog: None,
            restore: Vec::new(),
            error: None,
            today: Local::now().date_naive(),
            unlock: None,
            tray: Tray::new(&cc.egui_ctx),
            minimize_to_tray: true,
            hidden: false,
            settled: Arc::default(),
            marked: String::new(),
        };
        cc.egui_ctx.add_plugin(SelectionWatch(app.settled.clone()));
        cc.egui_ctx.add_plugin(InstantWheel);
        app.minimize_to_tray = app.storage.setting("minimize_to_tray").as_deref() != Some("0");
        if app.storage.lock_state() == Lock::Locked && !app.storage.unlock_remembered() {
            app.unlock = Some(UnlockPrompt::default());
        }
        app.reload();
        app.restore = app.conns.iter().filter(|c| c.active).map(|c| c.id).collect();
        app.selected = app.conns.first().map(|c| c.id);
        app
    }

    fn reload(&mut self) {
        match (self.storage.folders(), self.storage.connections()) {
            (Ok(f), Ok(c)) => {
                self.folders = f;
                self.conns = c;
            }
            (Err(e), _) | (_, Err(e)) => self.error = Some(format!("Ошибка чтения базы: {e:#}")),
        }
    }

    fn report<T>(&mut self, r: anyhow::Result<T>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                None
            }
        }
    }

    fn conn(&self, id: i64) -> Option<&ConnConfig> {
        self.conns.iter().find(|c| c.id == id)
    }

    fn conn_mut(&mut self, id: i64) -> Option<&mut ConnConfig> {
        self.conns.iter_mut().find(|c| c.id == id)
    }

    fn log_dir(cfg: &ConnConfig) -> PathBuf {
        // Windows не допускает точку или пробел в конце имени каталога.
        let dir = crate::logfile::file_stem(&cfg.effective_log_dir());
        let dir = dir.trim_end_matches(['.', ' ']);
        crate::app_dir()
            .join("logs")
            .join(if dir.is_empty() { "_" } else { dir })
    }

    fn status(&self, id: i64) -> ConnStatus {
        self.live
            .get(&id)
            .map(|l| l.status.clone())
            .unwrap_or(ConnStatus::Disconnected)
    }

    fn connect(&mut self, id: i64) {
        if self.live.contains_key(&id) {
            return;
        }
        let Some(cfg) = self.conn(id).cloned() else {
            return;
        };
        let session = self.next_session;
        self.next_session += 1;
        let tx = worker::spawn(
            self.rt.handle(),
            session,
            cfg.clone(),
            Self::log_dir(&cfg),
            self.sink.clone(),
        );
        self.live.insert(
            id,
            Live {
                session,
                tx,
                status: ConnStatus::Connecting,
            },
        );
        self.views.entry(id).or_default();
        if let Some(c) = self.conn_mut(id) {
            c.active = true;
        }
        let r = self.storage.set_active(id, true);
        self.report(r);
    }

    fn disconnect(&mut self, id: i64) {
        if let Some(l) = self.live.remove(&id) {
            let _ = l.tx.send(Cmd::Stop);
        }
        if let Some(c) = self.conn_mut(id) {
            c.active = false;
        }
        let r = self.storage.set_active(id, false);
        self.report(r);
    }

    fn save_conn(&mut self, cfg: &ConnConfig) -> Option<i64> {
        let r = self.storage.save_connection(cfg);
        let id = self.report(r)?;
        self.reload();
        Some(id)
    }

    fn add_channel(&mut self, id: i64, name: &str) {
        let name = name.trim().to_string();
        let Some(c) = self.conn_mut(id) else { return };
        if name.is_empty() || c.channels.contains(&name) {
            return;
        }
        c.channels.push(name.clone());
        c.channels.sort();
        let c = c.clone();
        self.save_conn(&c);
        if let Some(l) = self.live.get(&id) {
            let _ = l.tx.send(Cmd::Listen(name));
        }
    }

    fn remove_channel(&mut self, id: i64, name: &str) {
        let Some(c) = self.conn_mut(id) else { return };
        c.channels.retain(|x| x != name);
        let c = c.clone();
        self.save_conn(&c);
        if let Some(l) = self.live.get(&id) {
            let _ = l.tx.send(Cmd::Unlisten(name.to_string()));
        }
        if let Some(v) = self.views.get_mut(&id) {
            if v.tab.as_deref() == Some(name) {
                v.tab = None;
            }
        }
    }

    fn set_filters(&mut self, id: i64, filters: Vec<String>, includes: Vec<String>) {
        let Some(c) = self.conn_mut(id) else { return };
        c.filters = filters;
        c.includes = includes;
        let c = c.clone();
        self.save_conn(&c);
        if let Some(v) = self.views.get_mut(&id) {
            v.filter_version += 1;
        }
    }

    fn handle_events(&mut self) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Status(id, session, status) => {
                    if let Some(l) = self.live.get_mut(&id) {
                        if l.session == session {
                            if status == ConnStatus::Disconnected {
                                self.live.remove(&id);
                            } else {
                                l.status = status;
                            }
                        }
                    }
                }
                Event::Entry(id, e) => self.views.entry(id).or_default().push(e),
                Event::History(id, ch, entries) => {
                    self.views.entry(id).or_default().replace_channel(&ch, entries)
                }
                Event::TestResult(r) => {
                    if let Some(Dialog::Conn(d)) = &mut self.dialog {
                        d.testing = false;
                        d.test = Some(r);
                    }
                }
            }
        }
        let today = Local::now().date_naive();
        if today != self.today {
            self.today = today;
            for v in self.views.values_mut() {
                v.retain_day(today);
            }
        }
    }

    fn apply(&mut self, a: Action) {
        match a {
            Action::Select(id) => self.selected = Some(id),
            Action::Connect(id) => {
                self.selected = Some(id);
                self.connect(id)
            }
            Action::Disconnect(id) => self.disconnect(id),
            Action::Edit(id) => {
                if let Some(cfg) = self.conn(id).cloned() {
                    self.dialog = Some(Dialog::Conn(ConnDialog::new(cfg)));
                }
            }
            Action::NewConn(folder_id) => {
                self.dialog = Some(Dialog::Conn(ConnDialog::new(ConnConfig {
                    folder_id,
                    ..Default::default()
                })))
            }
            Action::Duplicate(id) => {
                if let Some(c) = self.conn(id) {
                    let mut cfg = c.clone();
                    cfg.id = 0;
                    cfg.active = false;
                    cfg.name = format!("{} (копия)", cfg.name);
                    // Порт моста занят оригиналом, поэтому в копии мост выключен.
                    cfg.bridge.enabled = false;
                    self.dialog = Some(Dialog::Conn(ConnDialog::new(cfg)));
                }
            }
            Action::NewFolder(parent_id) => {
                self.dialog = Some(Dialog::Folder(Folder {
                    id: 0,
                    parent_id,
                    name: String::new(),
                }))
            }
            Action::RenameFolder(id) => {
                if let Some(f) = self.folders.iter().find(|f| f.id == id) {
                    self.dialog = Some(Dialog::Folder(f.clone()));
                }
            }
            Action::Delete(t) => self.dialog = Some(Dialog::Delete(t)),
            Action::Master => {
                self.dialog = Some(Dialog::Master(MasterDialog {
                    remember: self.storage.is_remembered(),
                    ..Default::default()
                }))
            }
            Action::Move(Target::Conn(id), folder) => {
                if let Some(c) = self.conn_mut(id) {
                    c.folder_id = folder;
                    let c = c.clone();
                    self.save_conn(&c);
                }
            }
            Action::Move(Target::Folder(id), parent) => {
                if let Some(f) = self.folders.iter().find(|f| f.id == id) {
                    let mut f = f.clone();
                    f.parent_id = parent;
                    let r = self.storage.save_folder(&f);
                    self.report(r);
                    self.reload();
                }
            }
        }
    }

    // ---------- дерево подключений ----------

    fn folder_path(&self, id: i64) -> String {
        let mut parts = Vec::new();
        let mut cur = Some(id);
        while let Some(i) = cur {
            let Some(f) = self.folders.iter().find(|f| f.id == i) else {
                break;
            };
            parts.push(f.name.as_str());
            cur = f.parent_id;
            if parts.len() > 32 {
                break;
            }
        }
        parts.reverse();
        parts.join(" / ")
    }

    fn is_descendant(&self, folder: i64, ancestor: i64) -> bool {
        let mut cur = Some(folder);
        let mut guard = 0;
        while let Some(i) = cur {
            if i == ancestor {
                return true;
            }
            cur = self.folders.iter().find(|f| f.id == i).and_then(|f| f.parent_id);
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        false
    }

    fn move_menu(&self, ui: &mut egui::Ui, target: Target, actions: &mut Vec<Action>) {
        ui.menu_button("Переместить в", |ui| {
            if ui.button("(корень)").clicked() {
                actions.push(Action::Move(target, None));
            }
            let mut list: Vec<(String, i64)> = self
                .folders
                .iter()
                .filter(|f| match target {
                    Target::Folder(id) => !self.is_descendant(f.id, id),
                    Target::Conn(_) => true,
                })
                .map(|f| (self.folder_path(f.id), f.id))
                .collect();
            list.sort();
            for (path, id) in list {
                if ui.button(format!("📁 {path}")).clicked() {
                    actions.push(Action::Move(target, Some(id)));
                }
            }
        });
    }

    fn tree(&self, ui: &mut egui::Ui, parent: Option<i64>, actions: &mut Vec<Action>, depth: usize) {
        if depth > 32 {
            return;
        }
        for f in self.folders.iter().filter(|f| f.parent_id == parent) {
            let r = egui::CollapsingHeader::new(format!("📁 {}", f.name))
                .id_salt(("folder", f.id))
                .default_open(true)
                .show(ui, |ui| self.tree(ui, Some(f.id), actions, depth + 1));
            r.header_response.context_menu(|ui| {
                if ui.button("Новое подключение…").clicked() {
                    actions.push(Action::NewConn(Some(f.id)));
                }
                if ui.button("Новая папка…").clicked() {
                    actions.push(Action::NewFolder(Some(f.id)));
                }
                if ui.button("Переименовать…").clicked() {
                    actions.push(Action::RenameFolder(f.id));
                }
                self.move_menu(ui, Target::Folder(f.id), actions);
                ui.separator();
                if ui.button("Удалить папку").clicked() {
                    actions.push(Action::Delete(Target::Folder(f.id)));
                }
            });
        }
        for c in self.conns.iter().filter(|c| c.folder_id == parent) {
            let status = self.status(c.id);
            let (color, hint) = status_look(&status);
            let r = ui
                .horizontal(|ui| {
                    status_dot(ui, color).on_hover_text(hint);
                    ui.selectable_label(self.selected == Some(c.id), &c.name)
                })
                .inner;
            if r.clicked() {
                actions.push(Action::Select(c.id));
            }
            let connected = status != ConnStatus::Disconnected;
            if r.double_clicked() {
                actions.push(if connected {
                    Action::Disconnect(c.id)
                } else {
                    Action::Connect(c.id)
                });
            }
            r.context_menu(|ui| {
                if connected {
                    if ui.button("Отключить").clicked() {
                        actions.push(Action::Disconnect(c.id));
                    }
                } else if ui.button("Подключить").clicked() {
                    actions.push(Action::Connect(c.id));
                }
                if ui.button("Свойства…").clicked() {
                    actions.push(Action::Edit(c.id));
                }
                if ui.button("Дублировать…").clicked() {
                    actions.push(Action::Duplicate(c.id));
                }
                self.move_menu(ui, Target::Conn(c.id), actions);
                ui.separator();
                if ui.button("Удалить").clicked() {
                    actions.push(Action::Delete(Target::Conn(c.id)));
                }
            });
        }
    }

    fn left_panel(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            if ui.button("➕ Подключение").clicked() {
                actions.push(Action::NewConn(None));
            }
            if ui.button("📁 Папка").clicked() {
                actions.push(Action::NewFolder(None));
            }
            let (icon, hint) = match self.storage.lock_state() {
                Lock::NoPassword => ("🔓", "Пароли хранятся открытым текстом. Задать мастер-пароль"),
                _ => ("🔒", "Пароли зашифрованы мастер-паролем"),
            };
            if ui.button(icon).on_hover_text(hint).clicked() {
                actions.push(Action::Master);
            }
        });
        ui.separator();
        let r = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.tree(ui, None, actions, 0);
                // Свободное место под деревом — для контекстного меню корня.
                ui.allocate_response(ui.available_size(), egui::Sense::click())
            });
        r.inner.context_menu(|ui| {
            if ui.button("Новое подключение…").clicked() {
                actions.push(Action::NewConn(None));
            }
            if ui.button("Новая папка…").clicked() {
                actions.push(Action::NewFolder(None));
            }
        });
    }

    // ---------- центральная часть: лог ----------

    /// Берёт выделенный в строках лога текст для подсветки его вхождений.
    fn update_marked(&mut self, ctx: &egui::Context) {
        let has_selection = ctx
            .plugin::<egui::text_selection::LabelSelectionState>()
            .lock()
            .has_selection();
        let settled = self.settled.lock().unwrap().take();
        if !has_selection {
            self.marked.clear();
        } else if let Some(text) = settled {
            // Выделение через несколько строк или из одних пробелов не подсвечиваем.
            self.marked = if text.contains('\n') || text.trim().is_empty() {
                String::new()
            } else {
                text.to_lowercase()
            };
        }
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.selected else {
            ui.centered_and_justified(|ui| {
                ui.label("Создайте подключение кнопкой «➕ Подключение» слева");
            });
            return;
        };
        let Some(cfg) = self.conn(id).cloned() else {
            self.selected = None;
            return;
        };
        let status = self.status(id);
        let (color, hint) = status_look(&status);

        ui.horizontal(|ui| {
            ui.heading(&cfg.name);
            status_dot(ui, color);
            ui.label(RichText::new(hint).color(color));
            if cfg.bridge.enabled && status != ConnStatus::Disconnected {
                ui.label(
                    RichText::new(format!("· мост {}:{}", cfg.bridge.listen, cfg.bridge.port))
                        .color(GRAY),
                );
            }
        });
        ui.horizontal(|ui| {
            if status == ConnStatus::Disconnected {
                if ui.button("▶ Подключить").clicked() {
                    self.connect(id);
                }
            } else if ui.button("⏹ Отключить").clicked() {
                self.disconnect(id);
            }
            if ui.button("⚙ Свойства").clicked() {
                self.apply(Action::Edit(id));
            }
            let dir = Self::log_dir(&cfg);
            if ui
                .button("📂 Папка логов")
                .on_hover_text(dir.display().to_string())
                .clicked()
            {
                let _ = std::fs::create_dir_all(&dir);
                open_folder(&dir);
            }
            if ui
                .button("🗑 Очистить окно")
                .on_hover_text("Очищает только окно; файлы логов не трогаются")
                .clicked()
            {
                let v = self.views.entry(id).or_default();
                v.rows.clear();
                v.generation += 1;
            }
        });
        ui.separator();

        // Каналы и вкладки.
        let mut remove_channel = None;
        let mut add_channel = None;
        {
            let view = self.views.entry(id).or_default();
            ui.horizontal_wrapped(|ui| {
                ui.label("Каналы:");
                if ui.selectable_label(view.tab.is_none(), "Все").clicked() {
                    view.tab = None;
                }
                for ch in &cfg.channels {
                    let sel = view.tab.as_deref() == Some(ch.as_str());
                    if ui.selectable_label(sel, ch).clicked() {
                        view.tab = Some(Arc::from(ch.as_str()));
                    }
                    if ui
                        .small_button("✖")
                        .on_hover_text(format!("Перестать слушать «{ch}»"))
                        .clicked()
                    {
                        remove_channel = Some(ch.clone());
                    }
                }
                let r = ui.add(
                    egui::TextEdit::singleline(&mut view.channel_input)
                        .hint_text("новый канал")
                        .desired_width(140.0),
                );
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("➕").on_hover_text("Слушать канал").clicked() || enter {
                    add_channel = Some(std::mem::take(&mut view.channel_input));
                }
            });
        }
        if let Some(ch) = remove_channel {
            self.remove_channel(id, &ch);
        }
        if let Some(ch) = add_channel {
            self.add_channel(id, &ch);
        }

        // Фильтры и поиск.
        let mut filters = cfg.filters.clone();
        let mut includes = cfg.includes.clone();
        let changed = {
            let view = self.views.entry(id).or_default();
            let a = filter_row(
                ui,
                "Показывать только строки с:",
                "Если задано, показываются только строки, содержащие любую из подстрок. Регистр не учитывается.",
                &mut includes,
                &mut view.include_input,
            );
            let b = filter_row(
                ui,
                "Скрывать строки с:",
                "Строки, содержащие любую из подстрок, не показываются. Регистр не учитывается.",
                &mut filters,
                &mut view.filter_input,
            );
            a || b
        };
        if changed {
            self.set_filters(id, filters.clone(), includes.clone());
        }

        self.update_marked(ui.ctx());
        let marked = &self.marked;
        let view = self.views.entry(id).or_default();
        let total = view.rows.len();
        let cache_key = (view.tab.clone(), view.generation, view.filter_version);
        view.visible(&filters, &includes);
        let View {
            rows,
            cache: visible,
            search,
            tab,
            drawn,
            ..
        } = view;
        // Прилипание к концу лога срабатывает по размеру прошлого кадра, поэтому после
        // добавления строк нужен ещё один кадр, иначе последние строки останутся за краем.
        if *drawn != visible.len() {
            *drawn = visible.len();
            ui.ctx().request_repaint();
        }
        search.update(rows, visible, cache_key);
        search_row(ui, search);
        ui.separator();

        let all_tab = tab.is_none();
        let font = FontId::monospace(13.0);
        let row_h = ui.fonts_mut(|f| f.row_height(&font));
        let ch_width = cfg
            .channels
            .iter()
            .map(|c| c.chars().count())
            .max()
            .unwrap_or(0)
            .max(6);
        let dark = ui.visuals().dark_mode;
        let text_color = ui.visuals().text_color();

        ui.label(
            RichText::new(format!("Строк за сегодня: {total}, показано: {}", visible.len()))
                .small()
                .color(GRAY),
        );
        let mut copy: Option<String> = None;
        let query = search.query.to_lowercase();
        let current_pos = search.current.map(|c| search.matches[c]);
        let jump = search.scroll_to.take();
        // На кадре перехода к совпадению отключаем прилипание к концу, иначе оно перебьёт прокрутку.
        let mut area = egui::ScrollArea::both()
            .auto_shrink([false, false])
            .stick_to_bottom(jump.is_none());
        if let Some(pos) = jump {
            // Строку с совпадением ставим примерно в середину области.
            let step = row_h + ui.spacing().item_spacing.y;
            let offset = pos as f32 * step - ui.available_height() / 2.0;
            area = area.vertical_scroll_offset(offset.max(0.0));
        }
        area.show_rows(ui, row_h, visible.len(), |ui, range| {
                let first = range.start;
                for (k, &i) in visible[range].iter().enumerate() {
                    let row = &rows[i];
                    let is_current = current_pos == Some(first + k);
                    let mut job = LayoutJob::default();
                    let fmt = |c: Color32| TextFormat::simple(font.clone(), c);
                    job.append(
                        &row.entry.time.format("%H:%M:%S%.3f  ").to_string(),
                        0.0,
                        fmt(GRAY),
                    );
                    if all_tab {
                        let ch: &str = &row.entry.channel;
                        let (name, c) = if ch.is_empty() {
                            ("*", ORANGE)
                        } else {
                            (ch, channel_color(ch, dark))
                        };
                        job.append(&format!("{name:<ch_width$}  "), 0.0, fmt(c));
                    }
                    let c = if row.entry.channel.is_empty() { ORANGE } else { text_color };
                    append_highlighted(&mut job, &row.line, &query, marked, fmt(c), is_current);
                    let r = ui.add(
                        egui::Label::new(job)
                            .wrap_mode(egui::TextWrapMode::Extend)
                            .sense(egui::Sense::click()),
                    );
                    r.context_menu(|ui| {
                        if ui.button("Копировать строку").clicked() {
                            copy = Some(format_entry(&row.entry));
                        }
                        if ui.button("Копировать все показанные строки").clicked() {
                            copy = Some(
                                visible
                                    .iter()
                                    .map(|&i| format_entry(&rows[i].entry))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                            );
                        }
                    });
                }
            });
        if let Some(text) = copy {
            ui.ctx().copy_text(text);
        }
    }

    // ---------- диалоги ----------

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        self.dialog = match dialog {
            Dialog::Conn(d) => self.conn_dialog(ctx, d),
            Dialog::Folder(f) => self.folder_dialog(ctx, f),
            Dialog::Delete(t) => self.delete_dialog(ctx, t),
            Dialog::Master(d) => self.master_dialog(ctx, d),
        };
    }

    fn master_dialog(&mut self, ctx: &egui::Context, mut d: MasterDialog) -> Option<Dialog> {
        let state = self.storage.lock_state();
        let mut keep = true;
        let mut action: Option<Option<String>> = None;
        egui::Window::new("Мастер-пароль")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(match state {
                    Lock::NoPassword => "Сейчас пароли подключений хранятся в pglistener.db открытым текстом.",
                    _ => "Пароли подключений зашифрованы (AES-256-GCM, ключ из мастер-пароля через Argon2).",
                });
                ui.add_space(6.0);
                egui::Grid::new("master_grid").num_columns(2).show(ui, |ui| {
                    ui.label(if state == Lock::NoPassword { "Мастер-пароль" } else { "Новый пароль" });
                    ui.add(egui::TextEdit::singleline(&mut d.new).password(true).desired_width(220.0));
                    ui.end_row();
                    ui.label("Повтор");
                    ui.add(egui::TextEdit::singleline(&mut d.repeat).password(true).desired_width(220.0));
                    ui.end_row();
                });
                if cfg!(windows) {
                    ui.checkbox(&mut d.remember, "Запомнить на этом компьютере")
                        .on_hover_text("Пароль не будет спрашиваться при запуске под вашей учётной записью Windows. На другом компьютере его придётся ввести.");
                }
                if let Some(e) = &d.error {
                    ui.label(RichText::new(e).color(Color32::from_rgb(220, 60, 60)));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(if state == Lock::NoPassword { "Задать" } else { "Сменить" }).clicked() {
                        if d.new.is_empty() {
                            d.error = Some("Введите пароль".into());
                        } else if d.new != d.repeat {
                            d.error = Some("Пароли не совпадают".into());
                        } else {
                            action = Some(Some(d.new.clone()));
                        }
                    }
                    if state != Lock::NoPassword {
                        if ui.button("Снять пароль").on_hover_text("Пароли снова будут храниться открытым текстом").clicked() {
                            action = Some(None);
                        }
                        if cfg!(windows) && d.remember != self.storage.is_remembered() && ui.button("Применить «запомнить»").clicked() {
                            let r = if d.remember { self.storage.remember() } else { self.storage.forget() };
                            self.report(r);
                        }
                    }
                    if ui.button("Отмена").clicked() {
                        keep = false;
                    }
                });
            });
        if let Some(new) = action {
            let r = self.storage.set_master(new.as_deref(), d.remember);
            self.report(r);
            self.reload();
            return None;
        }
        keep.then_some(Dialog::Master(d))
    }

    /// Запрос мастер-пароля при запуске. Возвращает true, пока база заблокирована.
    fn unlock_prompt(&mut self, ctx: &egui::Context) -> bool {
        let Some(p) = &mut self.unlock else { return false };
        let mut submit = false;
        let mut reset = false;
        egui::Modal::new(egui::Id::unique("unlock_modal")).show(ctx, |ui| {
            ui.heading("Введите мастер-пароль");
            ui.label("Пароли подключений в pglistener.db зашифрованы.");
            ui.add_space(6.0);
            let r = ui.add(egui::TextEdit::singleline(&mut p.password).password(true).desired_width(260.0));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
            } else if ui.memory(|m| m.focused().is_none()) {
                r.request_focus();
            }
            if cfg!(windows) {
                ui.checkbox(&mut p.remember, "Запомнить на этом компьютере");
            }
            if p.error {
                ui.label(RichText::new("Неверный пароль").color(Color32::from_rgb(220, 60, 60)));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Открыть").clicked() {
                    submit = true;
                }
                if ui.button("Забыл пароль…").clicked() {
                    p.confirm_reset = true;
                }
            });
            if p.confirm_reset {
                ui.separator();
                ui.label("Все сохранённые пароли подключений будут стёрты, остальные настройки сохранятся. Продолжить?");
                ui.horizontal(|ui| {
                    if ui.button("Стереть пароли").clicked() {
                        reset = true;
                    }
                    if ui.button("Отмена").clicked() {
                        p.confirm_reset = false;
                    }
                });
            }
        });
        if submit {
            let (pw, remember) = (p.password.clone(), p.remember);
            match self.storage.unlock(&pw, remember) {
                Ok(true) => self.unlock = None,
                Ok(false) => {
                    if let Some(p) = &mut self.unlock {
                        p.error = true;
                        p.password.clear();
                    }
                }
                Err(e) => self.error = Some(format!("{e:#}")),
            }
        }
        if reset {
            let r = self.storage.reset_master();
            self.report(r);
            self.unlock = None;
        }
        if self.unlock.is_none() {
            self.reload();
            return false;
        }
        true
    }

    fn conn_dialog(&mut self, ctx: &egui::Context, mut d: Box<ConnDialog>) -> Option<Dialog> {
        let mut keep = true;
        let mut save = false;
        let title = if d.cfg.id == 0 { "Новое подключение" } else { "Свойства подключения" };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let c = &mut d.cfg;
                egui::Grid::new("conn_grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Имя");
                        ui.add(egui::TextEdit::singleline(&mut c.name).desired_width(300.0));
                        ui.end_row();
                        ui.label("Хост");
                        ui.add(egui::TextEdit::singleline(&mut c.host).desired_width(300.0))
                            .on_hover_text("При SSH-туннеле — адрес сервера БД со стороны SSH-сервера (обычно localhost)");
                        ui.end_row();
                        ui.label("Порт");
                        ui.add(egui::DragValue::new(&mut c.port).range(1..=65535));
                        ui.end_row();
                        ui.label("База данных");
                        ui.add(egui::TextEdit::singleline(&mut c.dbname).desired_width(300.0));
                        ui.end_row();
                        ui.label("Пользователь");
                        ui.add(egui::TextEdit::singleline(&mut c.user).desired_width(300.0));
                        ui.end_row();
                        ui.label("Пароль");
                        ui.add(
                            egui::TextEdit::singleline(&mut c.password)
                                .password(true)
                                .desired_width(300.0),
                        );
                        ui.end_row();
                        ui.label("SSL");
                        egui::ComboBox::from_id_salt("sslmode")
                            .selected_text(c.sslmode.as_str())
                            .show_ui(ui, |ui| {
                                for m in SslMode::ALL {
                                    ui.selectable_value(&mut c.sslmode, m, m.as_str());
                                }
                            });
                        ui.end_row();
                        ui.label("Каталог логов");
                        ui.vertical(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut c.log_dir)
                                    .hint_text(&c.name)
                                    .desired_width(300.0),
                            );
                            ui.label(
                                RichText::new(Self::log_dir(c).display().to_string())
                                    .small()
                                    .color(GRAY),
                            );
                        });
                        ui.end_row();
                    });

                ui.add_space(6.0);
                ui.checkbox(&mut c.ssh.enabled, "Подключаться через SSH-туннель");
                if c.ssh.enabled {
                    egui::Grid::new("ssh_grid")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            let s = &mut c.ssh;
                            ui.label("SSH-хост");
                            ui.add(egui::TextEdit::singleline(&mut s.host).desired_width(300.0));
                            ui.end_row();
                            ui.label("SSH-порт");
                            ui.add(egui::DragValue::new(&mut s.port).range(1..=65535));
                            ui.end_row();
                            ui.label("SSH-пользователь");
                            ui.add(egui::TextEdit::singleline(&mut s.user).desired_width(300.0));
                            ui.end_row();
                            ui.label("Аутентификация");
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut s.auth, SshAuth::Password, "Пароль");
                                ui.radio_value(&mut s.auth, SshAuth::Key, "Ключ");
                            });
                            ui.end_row();
                            match s.auth {
                                SshAuth::Password => {
                                    ui.label("SSH-пароль");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut s.password)
                                            .password(true)
                                            .desired_width(300.0),
                                    );
                                    ui.end_row();
                                }
                                SshAuth::Key => {
                                    ui.label("Файл ключа");
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::TextEdit::singleline(&mut s.key_path)
                                                .hint_text("OpenSSH/PEM, путь можно относительно exe")
                                                .desired_width(230.0),
                                        );
                                        if ui.button("Обзор…").clicked() {
                                            if let Some(p) = rfd::FileDialog::new().pick_file() {
                                                s.key_path = p.display().to_string();
                                            }
                                        }
                                    });
                                    ui.end_row();
                                    ui.label("Пароль ключа");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut s.key_passphrase)
                                            .password(true)
                                            .hint_text("если ключ зашифрован")
                                            .desired_width(300.0),
                                    );
                                    ui.end_row();
                                }
                            }
                        });
                }

                ui.add_space(6.0);
                ui.checkbox(&mut c.bridge.enabled, "Мост: пускать к БД клиентов из локальной сети")
                    .on_hover_text(
                        "PgListener принимает подключения на указанном порту и пробрасывает их к БД \
                         (через SSH-туннель, если он включён). Клиент указывает адрес этого ПК и порт \
                         моста, а логин и пароль к БД вводит свой. Мост работает, пока подключение активно.",
                    );
                if c.bridge.enabled {
                    egui::Grid::new("bridge_grid")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            let b = &mut c.bridge;
                            ui.label("Адрес");
                            ui.add(
                                egui::TextEdit::singleline(&mut b.listen)
                                    .hint_text("0.0.0.0")
                                    .desired_width(300.0),
                            )
                            .on_hover_text("0.0.0.0 — все сетевые интерфейсы, 127.0.0.1 — только этот ПК");
                            ui.end_row();
                            ui.label("Порт");
                            ui.add(egui::DragValue::new(&mut b.port).range(1..=65535));
                            ui.end_row();
                            ui.label("Разрешённые адреса");
                            ui.add(
                                egui::TextEdit::singleline(&mut d.allow_text)
                                    .hint_text("пусто — все адреса")
                                    .desired_width(300.0),
                            )
                            .on_hover_text("Адреса и подсети через запятую, например: 192.168.1.0/24, 10.0.0.5");
                            ui.end_row();
                        });
                }
                if let Some(e) = &d.error {
                    ui.label(RichText::new(e).color(Color32::from_rgb(220, 60, 60)));
                }

                ui.add_space(6.0);
                ui.separator();
                ui.label("Прослушиваемые каналы (изменения применяются сразу после сохранения, без переподключения):");
                ui.horizontal_wrapped(|ui| {
                    let mut remove = None;
                    for (i, ch) in c.channels.iter().enumerate() {
                        if ui.button(format!("{ch} ✖")).clicked() {
                            remove = Some(i);
                        }
                    }
                    if let Some(i) = remove {
                        c.channels.remove(i);
                    }
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut d.new_channel)
                            .hint_text("новый канал")
                            .desired_width(140.0),
                    );
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("➕").clicked() || enter {
                        let ch = std::mem::take(&mut d.new_channel).trim().to_string();
                        if !ch.is_empty() && !c.channels.contains(&ch) {
                            c.channels.push(ch);
                            c.channels.sort();
                        }
                    }
                });

                ui.separator();
                match &d.test {
                    _ if d.testing => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Проверка подключения…");
                        });
                    }
                    Some(Ok(v)) => {
                        ui.label(RichText::new(format!("✔ {v}")).color(GREEN));
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(format!("✖ {e}")).color(Color32::from_rgb(220, 60, 60)));
                    }
                    None => {}
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(!d.testing, egui::Button::new("Проверить подключение")).clicked() {
                        d.testing = true;
                        d.test = None;
                        worker::test(self.rt.handle(), d.cfg.clone(), self.sink.clone());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Отмена").clicked() {
                            keep = false;
                        }
                        if ui.button("Сохранить").clicked() {
                            save = true;
                        }
                    });
                });
            });

        if save {
            d.cfg.bridge.allow = crate::bridge::split_allow(&d.allow_text);
            d.cfg.bridge.listen = d.cfg.bridge.listen.trim().to_string();
            if d.cfg.bridge.listen.is_empty() {
                d.cfg.bridge.listen = "0.0.0.0".into();
            }
            if let Err(e) = crate::bridge::parse_allow(&d.cfg.bridge.allow) {
                d.error = Some(format!("Мост: {e}"));
                return Some(Dialog::Conn(d));
            }
            let cfg = d.cfg.clone();
            let old = self.conn(cfg.id).cloned();
            let old_channels = old.as_ref().map(|c| c.channels.clone()).unwrap_or_default();
            let bridge_changed = old.as_ref().is_some_and(|o| {
                let (a, b) = (&o.bridge, &cfg.bridge);
                (a.enabled, &a.listen, a.port, &a.allow) != (b.enabled, &b.listen, b.port, &b.allow)
            });
            if let Some(id) = self.save_conn(&cfg) {
                if let Some(l) = self.live.get(&id) {
                    for ch in cfg.channels.iter().filter(|c| !old_channels.contains(c)) {
                        let _ = l.tx.send(Cmd::Listen(ch.clone()));
                    }
                    for ch in old_channels.iter().filter(|c| !cfg.channels.contains(c)) {
                        let _ = l.tx.send(Cmd::Unlisten(ch.clone()));
                    }
                }
                if let Some(v) = self.views.get_mut(&id) {
                    if v.tab.as_ref().is_some_and(|t| !cfg.channels.iter().any(|c| **c == **t)) {
                        v.tab = None;
                    }
                }
                self.selected = Some(id);
                // Мост живёт вместе с подключением, поэтому новые настройки моста
                // применяем переподключением.
                if bridge_changed && self.live.contains_key(&id) {
                    self.disconnect(id);
                    self.connect(id);
                }
            }
            return None;
        }
        keep.then_some(Dialog::Conn(d))
    }

    fn folder_dialog(&mut self, ctx: &egui::Context, mut f: Folder) -> Option<Dialog> {
        let mut keep = true;
        let mut ok = false;
        let title = if f.id == 0 { "Новая папка" } else { "Переименовать папку" };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut f.name).hint_text("имя папки"));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    ok = true;
                } else if ui.memory(|m| m.focused().is_none()) {
                    r.request_focus();
                }
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        ok = true;
                    }
                    if ui.button("Отмена").clicked() {
                        keep = false;
                    }
                });
            });
        if ok && !f.name.trim().is_empty() {
            f.name = f.name.trim().to_string();
            let r = self.storage.save_folder(&f);
            self.report(r);
            self.reload();
            return None;
        }
        keep.then_some(Dialog::Folder(f))
    }

    fn delete_dialog(&mut self, ctx: &egui::Context, t: Target) -> Option<Dialog> {
        let text = match t {
            Target::Folder(id) => format!(
                "Удалить папку «{}»? Её содержимое переместится на уровень выше.",
                self.folder_path(id)
            ),
            Target::Conn(id) => format!(
                "Удалить подключение «{}»? Файлы логов останутся на диске.",
                self.conn(id).map(|c| c.name.as_str()).unwrap_or("")
            ),
        };
        let mut result = None;
        egui::Modal::new(egui::Id::unique("delete_modal")).show(ctx, |ui| {
            ui.label(text);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Удалить").clicked() {
                    result = Some(true);
                }
                if ui.button("Отмена").clicked() {
                    result = Some(false);
                }
            });
        });
        match result {
            None => Some(Dialog::Delete(t)),
            Some(false) => None,
            Some(true) => {
                let r = match t {
                    Target::Folder(id) => self.storage.delete_folder(id),
                    Target::Conn(id) => {
                        self.disconnect(id);
                        self.views.remove(&id);
                        if self.selected == Some(id) {
                            self.selected = None;
                        }
                        self.storage.delete_connection(id)
                    }
                };
                self.report(r);
                self.reload();
                None
            }
        }
    }

    fn restore_prompt(&mut self, ctx: &egui::Context) {
        if self.restore.is_empty() {
            return;
        }
        let names: Vec<String> = self
            .restore
            .iter()
            .filter_map(|id| self.conn(*id).map(|c| c.name.clone()))
            .collect();
        let mut result = None;
        egui::Modal::new(egui::Id::unique("restore_modal")).show(ctx, |ui| {
            ui.heading("Восстановить подключения?");
            ui.label("При прошлом закрытии были активны:");
            for n in &names {
                ui.label(format!("  • {n}"));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Да, подключить").clicked() {
                    result = Some(true);
                }
                if ui.button("Нет").clicked() {
                    result = Some(false);
                }
            });
        });
        match result {
            Some(true) => {
                for id in std::mem::take(&mut self.restore) {
                    self.connect(id);
                }
            }
            Some(false) => {
                self.restore.clear();
                let r = self.storage.clear_active();
                self.report(r);
                self.reload();
            }
            None => {}
        }
    }

    fn error_modal(&mut self, ctx: &egui::Context) {
        let Some(err) = &self.error else { return };
        let mut close = false;
        egui::Modal::new(egui::Id::unique("error_modal")).show(ctx, |ui| {
            ui.heading("Ошибка");
            ui.label(err);
            if ui.button("OK").clicked() {
                close = true;
            }
        });
        if close {
            self.error = None;
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events();
        while let Some(cmd) = self.tray.as_ref().and_then(Tray::poll) {
            match cmd {
                TrayCmd::Show => {
                    self.hidden = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayCmd::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        // Свёрнутое окно прячем в трей.
        if self.tray.is_some()
            && self.minimize_to_tray
            && !self.hidden
            && ctx.input(|i| i.viewport().minimized) == Some(true)
        {
            self.hidden = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut actions = Vec::new();
        egui::Panel::left("tree")
            .resizable(true)
            .default_size(260.0)
            .show(ui, |ui| {
                if self.tray.is_some()
                    && ui
                        .checkbox(&mut self.minimize_to_tray, "Сворачивать в трей")
                        .changed()
                {
                    let v = if self.minimize_to_tray { "1" } else { "0" };
                    let r = self.storage.set_setting("minimize_to_tray", v);
                    self.report(r);
                }
                self.left_panel(ui, &mut actions)
            });
        egui::CentralPanel::default().show(ui, |ui| self.central(ui));
        for a in actions {
            self.apply(a);
        }
        if !self.unlock_prompt(&ctx) {
            self.dialogs(&ctx);
            self.restore_prompt(&ctx);
        }
        self.error_modal(&ctx);
        // Чтобы смена суток обрабатывалась даже без новых сообщений.
        ctx.request_repaint_after(std::time::Duration::from_secs(30));
    }
}

fn status_look(s: &ConnStatus) -> (Color32, String) {
    match s {
        ConnStatus::Disconnected => (GRAY, "отключено".into()),
        ConnStatus::Connecting => (ORANGE, "подключение…".into()),
        ConnStatus::Connected => (GREEN, "подключено".into()),
        ConnStatus::Reconnecting(e) => (ORANGE, format!("переподключение: {e}")),
    }
}

/// Строка с «чипами» подстрок и полем для добавления новой. Возвращает true, если список изменился.
fn filter_row(
    ui: &mut egui::Ui,
    label: &str,
    hint: &str,
    list: &mut Vec<String>,
    input: &mut String,
) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(label).on_hover_text(hint);
        let mut i = 0;
        while i < list.len() {
            if ui.button(format!("{} ✖", list[i])).on_hover_text("Убрать").clicked() {
                list.remove(i);
                changed = true;
            } else {
                i += 1;
            }
        }
        let r = ui.add(
            egui::TextEdit::singleline(input)
                .hint_text("подстрока")
                .desired_width(160.0),
        );
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.button("➕").on_hover_text("Добавить").clicked() || enter) && !input.is_empty() {
            let f = std::mem::take(input);
            if !list.contains(&f) {
                list.push(f);
                changed = true;
            }
        }
        if !list.is_empty() && ui.button("Сбросить").clicked() {
            list.clear();
            changed = true;
        }
    });
    changed
}

/// Строка поиска: поле, переходы ▲/▼ и счётчик. Ctrl+F ставит фокус в поле.
fn search_row(ui: &mut egui::Ui, search: &mut Search) {
    ui.horizontal(|ui| {
        ui.label("Поиск:");
        let id = egui::Id::unique("log_search");
        let r = ui.add(
            egui::TextEdit::singleline(&mut search.query)
                .id(id)
                .hint_text("Ctrl+F")
                .return_key(None)
                .desired_width(220.0),
        );
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
            r.request_focus();
        }
        if r.has_focus() {
            // Enter — следующее (ниже), Shift+Enter — предыдущее; фокус остаётся в поле.
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter)) {
                search.step(false);
            } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                search.step(true);
            }
        }
        // Esc снимает фокус с поля, поэтому проверяем и только что потерянный фокус.
        if (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            search.query.clear();
        }
        let any = !search.matches.is_empty();
        if ui.add_enabled(any, egui::Button::new("⏶")).on_hover_text("Предыдущее (Shift+Enter)").clicked() {
            search.step(false);
        }
        if ui.add_enabled(any, egui::Button::new("⏷")).on_hover_text("Следующее (Enter)").clicked() {
            search.step(true);
        }
        if !search.query.is_empty() {
            let text = match search.current {
                Some(c) if any => format!("{} из {}", c + 1, search.matches.len()),
                _ => "нет совпадений".to_string(),
            };
            ui.label(RichText::new(text).color(GRAY));
            if ui.small_button("✖").on_hover_text("Очистить поиск (Esc)").clicked() {
                search.query.clear();
            }
        }
    });
}

/// Добавляет текст, подсвечивая вхождения поискового запроса `query` и выделенного
/// текста `marked` (оба в нижнем регистре). Подсветка поиска перекрывает выделение.
fn append_highlighted(
    job: &mut LayoutJob,
    text: &str,
    query: &str,
    marked: &str,
    fmt: TextFormat,
    current: bool,
) {
    let lower = text.to_lowercase();
    // Подсветка возможна, только если нижний регистр не меняет длину строки в байтах.
    if (query.is_empty() && marked.is_empty()) || lower.len() != text.len() {
        job.append(text, 0.0, fmt);
        return;
    }
    // Стиль каждого байта: 0 — обычный, 1 — выделенный текст, 2 — совпадение поиска.
    let mut style = vec![0u8; text.len()];
    for (pattern, level) in [(marked, 1u8), (query, 2u8)] {
        if pattern.is_empty() {
            continue;
        }
        for (start, m) in lower.match_indices(pattern) {
            let end = start + m.len();
            if text.is_char_boundary(start) && text.is_char_boundary(end) {
                style[start..end].fill(level);
            }
        }
    }
    let mut mark = fmt.clone();
    mark.background = Color32::from_rgb(140, 200, 255);
    mark.color = Color32::BLACK;
    let mut hl = mark.clone();
    hl.background = if current {
        Color32::from_rgb(255, 150, 30)
    } else {
        Color32::from_rgb(255, 225, 90)
    };
    let mut pos = 0;
    while pos < text.len() {
        let level = style[pos];
        let end = style[pos..]
            .iter()
            .position(|&l| l != level)
            .map_or(text.len(), |n| pos + n);
        let f = match level {
            0 => fmt.clone(),
            1 => mark.clone(),
            _ => hl.clone(),
        };
        job.append(&text[pos..end], 0.0, f);
        pos = end;
    }
}

fn status_dot(ui: &mut egui::Ui, color: Color32) -> egui::Response {
    let size = egui::vec2(10.0, ui.spacing().interact_size.y);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, color);
    resp
}

fn channel_color(ch: &str, dark: bool) -> Color32 {
    let h = ch.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    let hue = (h % 360) as f32 / 360.0;
    let (s, v) = if dark { (0.55, 0.95) } else { (0.75, 0.6) };
    egui::ecolor::Hsva::new(hue, s, v, 1.0).into()
}

fn format_entry(e: &LogEntry) -> String {
    let ch: &str = if e.channel.is_empty() { "*" } else { &e.channel };
    format!("{} {} {}", e.time.format("%Y-%m-%d %H:%M:%S%.3f"), ch, e.text)
}

fn open_folder(dir: &std::path::Path) {
    #[cfg(windows)]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(dir).spawn();
}

