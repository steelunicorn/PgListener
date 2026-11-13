use crate::model::*;
use crate::storage::{Lock, Storage};
use crate::tray::{Tray, TrayCmd};
use crate::worker::{self, EventSink};
use egui::text::LayoutJob;
use egui::{Color32, FontId, RichText, TextFormat};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;

const GREEN: Color32 = Color32::from_rgb(60, 180, 75);
const ORANGE: Color32 = Color32::from_rgb(230, 150, 30);
const GRAY: Color32 = Color32::from_rgb(140, 140, 140);

/// Цвета «хакерской» темы: приглушённый зелёный на зеленовато-чёрном, как в «Матрице».
const HACK_GREEN: Color32 = Color32::from_rgb(80, 188, 102);
const HACK_DIM: Color32 = Color32::from_rgb(48, 118, 64);
const HACK_LIME: Color32 = Color32::from_rgb(150, 196, 96);

/// Оформление интерфейса.
#[derive(Clone, Copy, PartialEq)]
enum UiTheme {
    System,
    Light,
    Dark,
    Hacker,
}

impl UiTheme {
    const ALL: [UiTheme; 4] = [UiTheme::System, UiTheme::Light, UiTheme::Dark, UiTheme::Hacker];

    fn key(self) -> &'static str {
        match self {
            UiTheme::System => "system",
            UiTheme::Light => "light",
            UiTheme::Dark => "dark",
            UiTheme::Hacker => "hacker",
        }
    }

    fn from_key(key: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|t| Some(t.key()) == key)
            .unwrap_or(UiTheme::System)
    }

    fn icon(self) -> RichText {
        match self {
            UiTheme::System => RichText::new("💻"),
            UiTheme::Light => RichText::new("☀"),
            UiTheme::Dark => RichText::new("🌙"),
            UiTheme::Hacker => RichText::new(">_").monospace().strong(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            UiTheme::System => "Как в системе",
            UiTheme::Light => "Светлая",
            UiTheme::Dark => "Тёмная",
            UiTheme::Hacker => "Хакерская",
        }
    }

    fn apply(self, ctx: &egui::Context) {
        let dark = if self == UiTheme::Hacker {
            hacker_visuals()
        } else {
            egui::Visuals::dark()
        };
        ctx.set_visuals_of(egui::Theme::Dark, dark);
        ctx.set_theme(match self {
            UiTheme::System => egui::ThemePreference::System,
            UiTheme::Light => egui::ThemePreference::Light,
            UiTheme::Dark | UiTheme::Hacker => egui::ThemePreference::Dark,
        });
    }
}

/// Тёмная тема egui, перекрашенная в приглушённый зелёный на зеленовато-чёрном фоне.
fn hacker_visuals() -> egui::Visuals {
    use egui::Stroke;
    let rgb = Color32::from_rgb;
    let panel = rgb(10, 18, 12);
    let raised = rgb(15, 27, 18);
    let sunken = rgb(6, 12, 8);
    let border = rgb(30, 58, 36);
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(HACK_GREEN);
    v.weak_text_color = Some(HACK_DIM);
    v.panel_fill = panel;
    v.window_fill = raised;
    v.extreme_bg_color = sunken;
    v.text_edit_bg_color = Some(sunken);
    v.faint_bg_color = raised;
    v.code_bg_color = raised;
    v.hyperlink_color = HACK_LIME;
    v.window_stroke = Stroke::new(1.0, border);
    v.selection.bg_fill = rgb(30, 80, 44);
    v.selection.stroke = Stroke::new(1.0, rgb(150, 220, 160));
    let w = &mut v.widgets;
    for (wv, bg, edge, fg) in [
        (&mut w.noninteractive, panel, border, HACK_DIM),
        (&mut w.inactive, rgb(20, 38, 25), border, HACK_GREEN),
        (&mut w.hovered, rgb(27, 52, 34), HACK_DIM, HACK_GREEN),
        (&mut w.active, rgb(35, 70, 44), HACK_GREEN, HACK_GREEN),
        (&mut w.open, rgb(24, 46, 30), HACK_DIM, HACK_GREEN),
    ] {
        wv.bg_fill = bg;
        wv.weak_bg_fill = bg;
        wv.bg_stroke = Stroke::new(1.0, edge);
        wv.fg_stroke = Stroke::new(wv.fg_stroke.width, fg);
    }
    v
}

/// Цвета строк лога для текущей темы.
#[derive(Clone, Copy)]
struct LogPalette {
    dark: bool,
    hacker: bool,
    text: Color32,
}

impl LogPalette {
    fn new(visuals: &egui::Visuals, hacker: bool) -> Self {
        Self {
            dark: visuals.dark_mode,
            hacker,
            text: if hacker { HACK_GREEN } else { visuals.text_color() },
        }
    }

    fn time(self) -> Color32 {
        if self.hacker { HACK_DIM } else { GRAY }
    }

    /// Строки без канала (служебные сообщения).
    fn special(self) -> Color32 {
        if self.hacker { HACK_LIME } else { ORANGE }
    }

    /// Фон вхождений выделенного текста.
    fn mark_bg(self) -> Color32 {
        if self.hacker {
            Color32::from_rgb(70, 160, 90)
        } else {
            Color32::from_rgb(140, 200, 255)
        }
    }

    /// Фон подсветки k-го поискового запроса; текст на нём чёрный.
    fn term_bg(self, k: usize) -> Color32 {
        const BRIGHT: [Color32; 6] = [
            Color32::from_rgb(255, 222, 90),
            Color32::from_rgb(150, 228, 140),
            Color32::from_rgb(255, 168, 196),
            Color32::from_rgb(255, 184, 110),
            Color32::from_rgb(200, 176, 255),
            Color32::from_rgb(120, 222, 205),
        ];
        const MUTED: [Color32; 6] = [
            Color32::from_rgb(196, 176, 70),
            Color32::from_rgb(110, 176, 104),
            Color32::from_rgb(190, 120, 150),
            Color32::from_rgb(200, 140, 80),
            Color32::from_rgb(150, 136, 200),
            Color32::from_rgb(90, 170, 156),
        ];
        let set = if self.hacker { &MUTED } else { &BRIGHT };
        set[k % set.len()]
    }

    /// Подложка строки с текущим совпадением поиска.
    fn current_row(self) -> Color32 {
        if self.hacker {
            Color32::from_rgb(28, 56, 34)
        } else if self.dark {
            Color32::from_rgb(70, 62, 30)
        } else {
            Color32::from_rgb(255, 238, 200)
        }
    }

    fn channel(self, ch: &str) -> Color32 {
        let h = ch.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
        if self.hacker {
            // Оттенки от жёлто-зелёного до бирюзового.
            let hue = (75 + h % 90) as f32 / 360.0;
            return egui::ecolor::Hsva::new(hue, 0.55, 0.78, 1.0).into();
        }
        let hue = (h % 360) as f32 / 360.0;
        let (s, v) = if self.dark { (0.55, 0.95) } else { (0.75, 0.6) };
        egui::ecolor::Hsva::new(hue, s, v, 1.0).into()
    }
}

/// Сколько последних сообщений хранится в окне одного подключения.
const MAX_ROWS: usize = 20_000;

/// Параметры, от которых зависят высоты строк (ключ кэша высот).
#[derive(Clone, Copy, PartialEq)]
struct HeightsKey {
    all_tab: bool,
    ch_width: usize,
    width_bits: u32,
}

struct Live {
    session: u64,
    tx: UnboundedSender<Cmd>,
    status: ConnStatus,
}

struct Row {
    /// Постоянный номер строки: по нему egui узнаёт строку при прокрутке,
    /// чтобы выделение мышью двигалось вместе с ней, а не оставалось на месте экрана.
    uid: u64,
    entry: LogEntry,
    lower: String,
    /// Текст сообщения как есть: переводы строк сохраняются и отображаются.
    line: String,
}

impl Row {
    fn new(entry: LogEntry) -> Self {
        // Разные виды переводов строк приводим к «\n», сами строки не склеиваем.
        let line = entry.text.replace("\r\n", "\n").replace('\r', "\n");
        static NEXT_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            uid: NEXT_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
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
    /// Каналы, строки которых не показываются.
    hidden: HashSet<Arc<str>>,
    /// Скрывать служебные сообщения приложения.
    hide_system: bool,
    /// Растёт при каждом изменении `hidden` и `hide_system`.
    channels_version: u64,
    filter_input: String,
    include_input: String,
    channel_input: String,
    cache_key: (u64, u64, u64),
    cache_seen: usize,
    cache: Vec<usize>,
    search: Search,
    /// Сколько строк было показано в прошлом кадре.
    drawn: usize,
    /// Высоты строк с переносами при текущих ширине и раскладке.
    heights: Vec<f32>,
    heights_key: Option<HeightsKey>,
}

/// Поиск по показанным строкам. Запросов может быть несколько, у каждого свой цвет.
#[derive(Default)]
struct Search {
    /// Текст в поле поиска (ещё не закреплённый Enter).
    query: String,
    /// Закреплённые запросы.
    terms: Vec<String>,
    /// Позиции совпавших строк в списке показанных.
    matches: Vec<usize>,
    /// Текущее совпадение (индекс в `matches`).
    current: Option<usize>,
    key: (String, u64, u64, u64, usize),
    /// Прокрутить к этой позиции на следующем кадре.
    scroll_to: Option<usize>,
}

impl Search {
    /// Запросы в нижнем регистре: закреплённые, затем текст из поля.
    fn patterns(&self) -> Vec<String> {
        let mut v: Vec<String> = self.terms.iter().map(|t| t.to_lowercase()).collect();
        if !self.query.is_empty() {
            v.push(self.query.to_lowercase());
        }
        v
    }

    /// Пересчитывает совпадения, если изменились запросы или набор показанных строк.
    fn update(&mut self, rows: &[Row], visible: &[usize], key: (u64, u64, u64)) {
        let pats = self.patterns();
        let sig = pats.join("\0");
        let hit = |i: usize| pats.iter().any(|p| rows[i].lower.contains(p.as_str()));
        if sig == self.key.0
            && (key.0, key.1, key.2) == (self.key.1, self.key.2, self.key.3)
            && visible.len() >= self.key.4
        {
            // Строки только добавились в конец: проверяем лишь новые, позиции старых не меняются.
            let seen = self.key.4;
            if visible.len() > seen && !pats.is_empty() {
                self.matches
                    .extend((seen..visible.len()).filter(|&pos| hit(visible[pos])));
                if self.current.is_none() {
                    self.current = self.matches.len().checked_sub(1);
                }
            }
            self.key.4 = visible.len();
            return;
        }
        let query_changed = sig != self.key.0;
        let current_row = self
            .current
            .and_then(|c| self.matches.get(c))
            .and_then(|&pos| visible.get(pos).copied());
        self.matches = visible
            .iter()
            .enumerate()
            .filter(|(_, &i)| hit(i))
            .map(|(pos, _)| pos)
            .collect();
        self.key = (sig, key.0, key.1, key.2, visible.len());
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

    /// Переходит к следующему (`forward`) или предыдущему совпадению только запроса `k`,
    /// считая от текущего совпадения; по краям переходит на другой конец.
    fn step_term(&mut self, rows: &[Row], visible: &[usize], k: usize, forward: bool) {
        let Some(term) = self.terms.get(k) else { return };
        let p = term.to_lowercase();
        let hits: Vec<usize> = (0..self.matches.len())
            .filter(|&m| {
                visible
                    .get(self.matches[m])
                    .is_some_and(|&i| rows[i].lower.contains(p.as_str()))
            })
            .collect();
        let (Some(&first), Some(&last)) = (hits.first(), hits.last()) else {
            return;
        };
        let next = match self.current {
            None if forward => first,
            None => last,
            Some(c) if forward => hits.iter().copied().find(|&m| m > c).unwrap_or(first),
            Some(c) => hits.iter().rev().copied().find(|&m| m < c).unwrap_or(last),
        };
        self.current = Some(next);
        self.scroll_to = Some(self.matches[next]);
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
            self.heights.clear();
            self.heights_key = None;
        } else {
            self.rows.push(Row::new(e));
        }
        self.trim();
    }

    /// Добавляет историю канала к уже показанным строкам; повторы пропускаются.
    fn merge_channel(&mut self, channel: &str, entries: Vec<LogEntry>) {
        let mut known: HashSet<(chrono::NaiveDateTime, String)> = self
            .rows
            .iter()
            .filter(|r| &*r.entry.channel == channel)
            .map(|r| (r.entry.time, r.entry.text.clone()))
            .collect();
        let mut added = 0;
        for e in entries {
            if known.insert((e.time, e.text.clone())) {
                self.rows.push(Row::new(e));
                added += 1;
            }
        }
        if added > 0 {
            self.rows.sort_by_key(|r| r.entry.time);
            self.generation += 1;
            self.heights.clear();
            self.heights_key = None;
        }
        self.trim();
    }

    /// Оставляет последние `MAX_ROWS` строк, сдвигая индексы кэшей.
    ///
    /// `generation` при этом не меняется: показанные строки — подмножество прежних,
    /// поэтому инкрементальные кэши (`visible` и поиск) остаются верными, и на каждом
    /// новом сообщении не приходится заново просматривать весь лог.
    fn trim(&mut self) {
        let extra = self.rows.len().saturating_sub(MAX_ROWS);
        if extra == 0 {
            return;
        }
        self.rows.drain(..extra);
        if self.heights.len() >= extra {
            self.heights.drain(..extra);
        } else {
            self.heights.clear();
            self.heights_key = None;
        }
        let old_visible = self.cache.len();
        self.cache.retain(|&i| i >= extra);
        for i in &mut self.cache {
            *i -= extra;
        }
        self.cache_seen = self.cache_seen.saturating_sub(extra).min(self.rows.len());
        let removed = old_visible - self.cache.len();
        if removed > 0 {
            // Текущее совпадение могло съехать вместе со строками: считаем, сколько
            // совпадений пропало перед ним, и сдвигаем индекс.
            let before = self
                .search
                .matches
                .iter()
                .filter(|&&p| p < removed)
                .count();
            self.search.matches.retain(|&p| p >= removed);
            for p in &mut self.search.matches {
                *p -= removed;
            }
            self.search.key.4 = self.search.key.4.saturating_sub(removed);
            self.search.current = self
                .search
                .current
                .map(|c| c.saturating_sub(before))
                .filter(|&c| c < self.search.matches.len());
        }
    }

    /// Скрывает или снова показывает строки канала.
    fn set_hidden(&mut self, ch: &str, hide: bool) {
        let changed = if hide {
            self.hidden.insert(Arc::from(ch))
        } else {
            self.hidden.remove(ch)
        };
        if changed {
            self.channels_version += 1;
        }
    }

    fn show_all_channels(&mut self) {
        if !self.hidden.is_empty() {
            self.hidden.clear();
            self.channels_version += 1;
        }
    }

    /// Оставляет видимым только канал `ch`.
    fn show_only(&mut self, channels: &[String], ch: &str) {
        self.hidden = channels
            .iter()
            .filter(|c| c.as_str() != ch)
            .map(|c| Arc::from(c.as_str()))
            .collect();
        self.channels_version += 1;
    }

    /// Забывает скрытые каналы, которых больше нет в настройках подключения.
    fn retain_hidden(&mut self, channels: &[String]) {
        let before = self.hidden.len();
        self.hidden.retain(|h| channels.iter().any(|c| c.as_str() == &**h));
        if self.hidden.len() != before {
            self.channels_version += 1;
        }
    }

    fn visible(&mut self, filters: &[String], includes: &[String], show_system: bool) -> &[usize] {
        if self.hide_system == show_system {
            self.hide_system = !show_system;
            self.channels_version += 1;
        }
        let key = (self.channels_version, self.generation, self.filter_version);
        if key != self.cache_key {
            self.cache_key = key;
            self.cache.clear();
            self.cache_seen = 0;
        }
        let lower: Vec<String> = filters.iter().map(|f| f.to_lowercase()).collect();
        let include: Vec<String> = includes.iter().map(|f| f.to_lowercase()).collect();
        for (i, r) in self.rows.iter().enumerate().skip(self.cache_seen) {
            if self.hidden.contains(&*r.entry.channel)
                || (self.hide_system && r.entry.channel.is_empty())
            {
                continue;
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
    unlock: Option<UnlockPrompt>,
    tray: Option<Tray>,
    minimize_to_tray: bool,
    theme: UiTheme,
    /// Показывать служебные сообщения приложения в логе.
    show_system: bool,
    /// В этом кадре нажат Esc при открытом диалоге: диалог закрывается как по «Отмена».
    esc: bool,
    /// Высоты верхних и нижних блоков левой [0] и центральной [1] панелей:
    /// блоки тянутся до большей из двух, чтобы разделители шли на одном уровне.
    top_h: [f32; 2],
    /// Высота строки каналов внизу центральной панели (используется только [1]).
    bottom_h: [f32; 2],
    hidden: bool,
    /// Выделение, о котором egui сообщил в конце прошлого кадра.
    settled: Arc<Mutex<Option<String>>>,
    /// Выделенный в логе текст (в нижнем регистре), его вхождения подсвечиваются.
    marked: String,
    /// Был щелчок или Esc: если после него выделения нет, подсветку снимаем.
    /// Само по себе исчезновение выделения (строка ушла за край при прокрутке) подсветку не снимает.
    deselect_pending: bool,
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
            unlock: None,
            tray: Tray::new(&cc.egui_ctx),
            minimize_to_tray: true,
            theme: UiTheme::System,
            show_system: true,
            esc: false,
            top_h: [0.0; 2],
            bottom_h: [0.0; 2],
            hidden: false,
            settled: Arc::default(),
            marked: String::new(),
            deselect_pending: false,
        };
        cc.egui_ctx.add_plugin(SelectionWatch(app.settled.clone()));
        cc.egui_ctx.add_plugin(InstantWheel);
        app.minimize_to_tray = app.storage.setting("minimize_to_tray").as_deref() != Some("0");
        app.theme = UiTheme::from_key(app.storage.setting("theme").as_deref());
        app.show_system = app.storage.setting("show_system").as_deref() != Some("0");
        app.theme.apply(&cc.egui_ctx);
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
            v.set_hidden(name, false);
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
                    self.views.entry(id).or_default().merge_channel(&ch, entries)
                }
                Event::TestResult(r) => {
                    if let Some(Dialog::Conn(d)) = &mut self.dialog {
                        d.testing = false;
                        d.test = Some(r);
                    }
                }
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

    fn left_buttons(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
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
    }

    /// Кнопка ⚙ с окошком настроек приложения.
    fn settings_button(&mut self, ui: &mut egui::Ui) {
        let resp = ui
            .add(egui::Button::new(RichText::new("⚙").size(17.0)).frame(false))
            .on_hover_text("Настройки");
        egui::Popup::from_toggle_button_response(&resp)
            .align(egui::RectAlign::BOTTOM_END)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| self.settings_popup(ui));
    }

    fn settings_popup(&mut self, ui: &mut egui::Ui) {
        let mut theme = self.theme;
        let mut tray = self.minimize_to_tray;
        let mut system = self.show_system;
        ui.set_width(300.0);
        setting_row(ui, "Тема", |ui| {
            let options = UiTheme::ALL.map(|t| (t, t.icon(), t.label()));
            segmented(ui, &mut theme, &options);
        });
        if self.tray.is_some() {
            setting_row(ui, "Сворачивать в трей", |ui| {
                toggle_switch(ui, &mut tray);
            });
        }
        setting_row(ui, "Системные сообщения", |ui| {
            toggle_switch(ui, &mut system)
                .on_hover_text("Сообщения самого PgListener (подключение, ошибки) в окне лога");
        });
        if theme != self.theme {
            self.theme = theme;
            theme.apply(ui.ctx());
            let r = self.storage.set_setting("theme", theme.key());
            self.report(r);
        }
        if tray != self.minimize_to_tray {
            self.minimize_to_tray = tray;
            let r = self.storage.set_setting("minimize_to_tray", if tray { "1" } else { "0" });
            self.report(r);
        }
        if system != self.show_system {
            self.show_system = system;
            let r = self.storage.set_setting("show_system", if system { "1" } else { "0" });
            self.report(r);
        }
    }

    fn left_tree(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
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
        if ctx.input(|i| i.pointer.primary_pressed() || i.key_pressed(egui::Key::Escape)) {
            self.deselect_pending = true;
        }
        let settled = self.settled.lock().unwrap().take();
        if let Some(text) = settled {
            // Выделение через несколько строк или из одних пробелов не подсвечиваем.
            self.marked = if text.contains('\n') || text.trim().is_empty() {
                String::new()
            } else {
                text.to_lowercase()
            };
            self.deselect_pending = false;
        } else if !has_selection && self.deselect_pending {
            // egui сбрасывает выделение и когда выделенная строка уходит за край при прокрутке,
            // поэтому подсветку снимаем только после щелчка или Esc.
            self.marked.clear();
            self.deselect_pending = false;
        }
    }

    /// Выбранное подключение, если оно есть.
    fn selected_conn(&mut self) -> Option<(i64, ConnConfig)> {
        let id = self.selected?;
        let cfg = self.conn(id).cloned();
        if cfg.is_none() {
            self.selected = None;
        }
        cfg.map(|c| (id, c))
    }

    /// Заголовок подключения и кнопки управления им.
    fn central_header(&mut self, ui: &mut egui::Ui) {
        let conn = self.selected_conn();
        let status = conn.as_ref().map(|(id, _)| self.status(*id));
        ui.horizontal(|ui| {
            if let (Some((_, cfg)), Some(status)) = (&conn, &status) {
                let (color, hint) = status_look(status);
                ui.heading(&cfg.name);
                status_dot(ui, color);
                ui.label(RichText::new(hint).color(color));
                if cfg.bridge.enabled && *status != ConnStatus::Disconnected {
                    ui.label(
                        RichText::new(format!("· мост {}:{}", cfg.bridge.listen, cfg.bridge.port))
                            .color(GRAY),
                    );
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.settings_button(ui)
            });
        });
        let (Some((id, cfg)), Some(status)) = (conn, status) else {
            return;
        };
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
                v.heights.clear();
                v.heights_key = None;
            }
        });
    }

    /// Переключатели каналов: какие показывать в логе. Здесь же добавление и удаление каналов.
    fn channels_row(&mut self, ui: &mut egui::Ui) {
        let Some((id, cfg)) = self.selected_conn() else {
            return;
        };
        let mut remove_channel = None;
        let mut add_channel = None;
        {
            let view = self.views.entry(id).or_default();
            ui.horizontal_wrapped(|ui| {
                if ui
                    .selectable_label(view.hidden.is_empty(), "Все")
                    .on_hover_text("Показывать все каналы")
                    .clicked()
                {
                    view.show_all_channels();
                }
                for ch in &cfg.channels {
                    let on = !view.hidden.contains(ch.as_str());
                    match channel_chip(ui, ch, on) {
                        Some(ChipAction::Toggle) => view.set_hidden(ch, on),
                        Some(ChipAction::Only) => view.show_only(&cfg.channels, ch),
                        Some(ChipAction::Remove) => remove_channel = Some(ch.clone()),
                        None => {}
                    }
                }
                let r = ui.add(
                    egui::TextEdit::singleline(&mut view.channel_input)
                        .hint_text("новый канал")
                        .desired_width(140.0),
                );
                if enter_pressed(ui, &r) && !view.channel_input.trim().is_empty() {
                    add_channel = Some(std::mem::take(&mut view.channel_input));
                    r.request_focus();
                }
            });
        }
        if let Some(ch) = remove_channel {
            self.remove_channel(id, &ch);
        }
        if let Some(ch) = add_channel {
            self.add_channel(id, &ch);
        }
    }

    /// Фильтры, поиск и сам лог.
    fn central(&mut self, ui: &mut egui::Ui) {
        let Some((id, cfg)) = self.selected_conn() else {
            ui.centered_and_justified(|ui| {
                ui.label("Создайте подключение кнопкой «➕ Подключение» слева");
            });
            return;
        };

        // Фильтры и поиск: подписи слева, за вертикальной чертой — поля и применённые значения.
        let mut filters = cfg.filters.clone();
        let mut includes = cfg.includes.clone();
        let labels = ["Показывать вхождения", "Скрывать вхождения", "Поиск"];
        let label_w = labels
            .iter()
            .map(|l| {
                ui.painter()
                    .layout_no_wrap(l.to_string(), egui::TextStyle::Body.resolve(ui.style()), Color32::PLACEHOLDER)
                    .size()
                    .x
            })
            .fold(0.0, f32::max);
        let block_top = ui.cursor().top();
        let changed = {
            let view = self.views.entry(id).or_default();
            let a = labeled_row(
                ui,
                label_w,
                labels[0],
                "Если задано, показываются только строки, содержащие любую из подстрок. Регистр не учитывается.",
                |ui| filter_items(ui, &mut includes, &mut view.include_input),
            );
            let b = labeled_row(
                ui,
                label_w,
                labels[1],
                "Строки, содержащие любую из подстрок, не показываются. Регистр не учитывается.",
                |ui| filter_items(ui, &mut filters, &mut view.filter_input),
            );
            a || b
        };
        if changed {
            self.set_filters(id, filters.clone(), includes.clone());
        }

        self.update_marked(ui.ctx());
        let pal = LogPalette::new(ui.visuals(), self.theme == UiTheme::Hacker);
        let show_system = self.show_system;
        let marked = &self.marked;
        let view = self.views.entry(id).or_default();
        let total = view.rows.len();
        view.visible(&filters, &includes, show_system);
        let cache_key = (view.channels_version, view.generation, view.filter_version);
        let View {
            rows,
            cache: visible,
            search,
            hidden,
            drawn,
            heights,
            heights_key,
            ..
        } = view;
        // Прилипание к концу лога срабатывает по размеру прошлого кадра, поэтому после
        // добавления строк нужен ещё один кадр, иначе последние строки останутся за краем.
        if *drawn != visible.len() {
            *drawn = visible.len();
            ui.ctx().request_repaint();
        }
        search.update(rows, visible, cache_key);
        labeled_row(
            ui,
            label_w,
            labels[2],
            "Enter закрепляет запрос, у каждого запроса свой цвет. Enter в пустом поле — следующее совпадение.",
            |ui| search_items(ui, search, pal, rows, visible),
        );
        // Вертикальная черта между подписями и полями.
        let x = ui.max_rect().left() + label_w + LABEL_GAP / 2.0;
        let bottom = ui.cursor().top() - ui.spacing().item_spacing.y;
        ui.painter()
            .vline(x, block_top..=bottom, ui.visuals().widgets.noninteractive.bg_stroke);
        ui.separator();

        // Колонка канала нужна, если показывается больше одного канала.
        let all_tab = cfg
            .channels
            .iter()
            .filter(|c| !hidden.contains(c.as_str()))
            .count()
            != 1;
        let ch_width = cfg
            .channels
            .iter()
            .map(|c| c.chars().count())
            .max()
            .unwrap_or(0)
            .max(6);
        let counter = if total >= MAX_ROWS {
            format!(
                "Строк в окне: {} (хранятся последние), показано: {}",
                fmt_count(total),
                visible.len()
            )
        } else {
            format!(
                "Строк в окне: {}, показано: {}",
                fmt_count(total),
                visible.len()
            )
        };
        ui.label(RichText::new(counter).small().color(GRAY));

        let mut copy: Option<String> = None;
        let patterns = search.patterns();
        let current_pos = search.current.map(|c| search.matches[c]);
        let jump = search.scroll_to.take();
        // На кадре перехода к совпадению отключаем прилипание к концу, иначе оно перебьёт прокрутку.
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .stick_to_bottom(jump.is_none())
            .show_viewport(ui, |ui, viewport| {
                // Ширина берётся до отрисовки: полоса прокрутки всегда занимает место,
                // поэтому перенос не «дёргается» при появлении/исчезновении строк.
                let width = ui.available_width().max(1.0);
                let key = HeightsKey {
                    all_tab,
                    ch_width,
                    width_bits: width.to_bits(),
                };
                ensure_heights(heights, heights_key, rows, ui, key, pal);
                let spacing = ui.spacing().item_spacing.y;
                let count = visible.len();
                let mut total_h = spacing * count as f32;
                for &i in visible.iter() {
                    total_h += heights[i];
                }
                ui.set_height(total_h);

                let left = ui.max_rect().left();
                let top = ui.max_rect().top();

                // Первая и последняя строки, попавшие в видимую область.
                let mut first = 0;
                let mut first_y = 0.0f32;
                while first < count {
                    let h = heights[visible[first]] + spacing;
                    if first_y + h > viewport.min.y {
                        break;
                    }
                    first_y += h;
                    first += 1;
                }
                let mut last = first;
                let mut y = first_y;
                while last < count {
                    if y >= viewport.max.y {
                        break;
                    }
                    y += heights[visible[last]] + spacing;
                    last += 1;
                }

                if let Some(pos) = jump {
                    if pos < count {
                        let mut y = 0.0f32;
                        for &i in &visible[..pos] {
                            y += heights[i] + spacing;
                        }
                        let h = heights[visible[pos]];
                        // Прямоугольник — в экранных координатах содержимого.
                        ui.scroll_to_rect(
                            egui::Rect::from_min_size(
                                egui::pos2(left, top + y),
                                egui::vec2(width, h),
                            ),
                            Some(egui::Align::Center),
                        );
                    }
                }

                let mut y = first_y;
                for pos in first..last {
                    let i = visible[pos];
                    let h = heights[i];
                    let rect =
                        egui::Rect::from_min_size(egui::pos2(left, top + y), egui::vec2(width, h));
                    y += h + spacing;
                    if current_pos == Some(pos) {
                        // Текущее совпадение поиска — подложка под всей строкой.
                        ui.painter().rect_filled(
                            rect.expand2(egui::vec2(2.0, 1.0)),
                            3.0,
                            pal.current_row(),
                        );
                    }
                    let job = row_job(&rows[i], all_tab, ch_width, pal, &patterns, marked, width);
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .scope_id(egui::Id::unique(("log_row", rows[i].uid)))
                            .max_rect(rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                        |ui| {
                            let r = ui.add(
                                egui::Label::new(job)
                                    .wrap_mode(egui::TextWrapMode::Wrap)
                                    .sense(egui::Sense::click()),
                            );
                            r.context_menu(|ui| {
                                if ui.button("Копировать строку").clicked() {
                                    copy = Some(format_entry(&rows[i].entry));
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
                        },
                    );
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
        if std::mem::take(&mut self.esc) {
            keep = false;
        }
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
        if p.confirm_reset && std::mem::take(&mut self.esc) {
            p.confirm_reset = false;
        }
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
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut d.new_channel)
                            .hint_text("новый канал, Enter")
                            .desired_width(140.0),
                    );
                    if enter_pressed(ui, &r) && !d.new_channel.trim().is_empty() {
                        let ch = std::mem::take(&mut d.new_channel).trim().to_string();
                        if !c.channels.contains(&ch) {
                            c.channels.push(ch);
                            c.channels.sort();
                        }
                        r.request_focus();
                    }
                    let mut remove = None;
                    for (i, ch) in c.channels.iter().enumerate() {
                        let (_, x) = x_chip(ui, ch, "", "Убрать канал", plain_chip_colors);
                        if x.clicked() {
                            remove = Some(i);
                        }
                    }
                    if let Some(i) = remove {
                        c.channels.remove(i);
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

        if std::mem::take(&mut self.esc) {
            keep = false;
            save = false;
        }
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
                    v.retain_hidden(&cfg.channels);
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
        if std::mem::take(&mut self.esc) {
            keep = false;
            ok = false;
        }
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
        if std::mem::take(&mut self.esc) {
            result = Some(false);
        }
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
        if std::mem::take(&mut self.esc) {
            result = Some(false);
        }
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
        // Esc закрывает верхний открытый диалог так же, как «Отмена».
        let any_dialog = self.error.is_some()
            || self.dialog.is_some()
            || self.unlock.is_some()
            || !self.restore.is_empty();
        if any_dialog && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if self.error.is_some() {
                self.error = None;
            } else {
                self.esc = true;
            }
        }
        let mut actions = Vec::new();
        // Одинаковые поля у обеих панелей, чтобы их блоки начинались на одной высоте.
        let margin = egui::Margin::same(8);
        let mut top_h = self.top_h;
        let mut bottom_h = self.bottom_h;
        egui::Panel::left("tree")
            .resizable(false)
            .exact_size(260.0)
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(margin))
            .show(ui, |ui| {
                even_block(ui, &mut top_h, 0, |ui| self.left_buttons(ui, &mut actions));
                ui.separator();
                self.left_tree(ui, &mut actions);
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(margin))
            .show(ui, |ui| {
                even_block(ui, &mut top_h, 1, |ui| self.central_header(ui));
                ui.separator();
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    even_block(ui, &mut bottom_h, 1, |ui| self.channels_row(ui));
                    ui.separator();
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        self.central(ui)
                    });
                });
            });
        self.top_h = top_h;
        self.bottom_h = bottom_h;
        for a in actions {
            self.apply(a);
        }
        if !self.unlock_prompt(&ctx) {
            self.dialogs(&ctx);
            self.restore_prompt(&ctx);
        }
        self.error_modal(&ctx);
        self.esc = false;
    }
}

/// Блок, который тянется до высоты такого же блока соседней панели.
/// `heights` — высоты содержимого блоков обеих панелей, `slot` — номер этой панели.
fn even_block(ui: &mut egui::Ui, heights: &mut [f32; 2], slot: usize, add: impl FnOnce(&mut egui::Ui)) {
    let target = heights[0].max(heights[1]);
    let width = ui.available_width();
    ui.allocate_ui_with_layout(
        egui::vec2(width, target),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_min_width(width);
            let h = ui.scope(add).response.rect.height();
            if (h - heights[slot]).abs() > 0.5 {
                heights[slot] = h;
                ui.ctx().request_repaint();
            }
            // Дотягиваем блок до общей высоты, считая от его верха
            // (add_space и set_min_height отсчитывают от курсора и добавляют отступ).
            ui.expand_to_include_y(ui.max_rect().top() + target);
        },
    );
}

enum ChipAction {
    Toggle,
    Only,
    Remove,
}

/// Чип с текстом и крестиком внутри. `colors(ui, hovered)` даёт фон, цвет текста и рамку.
/// Возвращает ответы самого чипа и зоны крестика.
fn x_chip(
    ui: &mut egui::Ui,
    name: &str,
    hint: &str,
    x_hint: &str,
    colors: impl FnOnce(&egui::Ui, bool) -> (Color32, Color32, egui::Stroke),
) -> (egui::Response, egui::Response) {
    let (resp, mut icons) = icon_chip(ui, name, hint, &[("✖", x_hint)], colors);
    (resp, icons.remove(0))
}

/// Чип с текстом и кнопками-значками у правого края (последний — крестик удаления).
/// Зоны значков перекрывают чип, поэтому щелчок по ним не доходит до самого чипа.
/// Возвращает ответ чипа и ответы значков в том же порядке.
fn icon_chip(
    ui: &mut egui::Ui,
    name: &str,
    hint: &str,
    icons: &[(&str, &str)],
    colors: impl FnOnce(&egui::Ui, bool) -> (Color32, Color32, egui::Stroke),
) -> (egui::Response, Vec<egui::Response>) {
    let pad = ui.spacing().button_padding;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let painter = ui.painter();
    let name_g = painter.layout_no_wrap(name.to_owned(), font.clone(), Color32::PLACEHOLDER);
    let icon_g: Vec<_> = icons
        .iter()
        .map(|(icon, _)| painter.layout_no_wrap(icon.to_string(), font.clone(), Color32::PLACEHOLDER))
        .collect();
    let gap = 6.0;
    // Ширина зоны значка: сам значок и по половине промежутка с каждой стороны.
    let zone_w = |g: &std::sync::Arc<egui::Galley>| g.size().x + gap;
    let icons_w: f32 = icon_g.iter().map(zone_w).sum();
    let text_h = icon_g.iter().map(|g| g.size().y).fold(name_g.size().y, f32::max);
    let size = egui::vec2(
        pad.x + name_g.size().x + gap / 2.0 + icons_w + pad.x - gap / 2.0,
        (text_h + 2.0 * pad.y).max(ui.spacing().interact_size.y),
    );
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let mut x = rect.right() - pad.x + gap / 2.0 - icons_w;
    let mut zones = Vec::with_capacity(icons.len());
    for (k, ((_, icon_hint), g)) in icons.iter().zip(&icon_g).enumerate() {
        // Последняя зона (крестик) захватывает и правое поле чипа.
        let right = if k + 1 == icons.len() { rect.right() } else { x + zone_w(g) };
        let zone = egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(right, rect.bottom()));
        x += zone_w(g);
        let r = ui
            .interact(zone, resp.id.with(("icon", k)), egui::Sense::click())
            .on_hover_text(*icon_hint);
        zones.push(r);
    }
    let resp = if hint.is_empty() { resp } else { resp.on_hover_text(hint) };
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered() || zones.iter().any(|z| z.hovered());
        let (fill, fg, stroke) = colors(ui, hovered);
        let radius = ui.visuals().widgets.inactive.corner_radius;
        let painter = ui.painter();
        painter.rect(rect, radius, fill, stroke, egui::StrokeKind::Inside);
        let text_y = rect.center().y - name_g.size().y / 2.0;
        painter.galley(egui::pos2(rect.left() + pad.x, text_y), name_g, fg);
        let last = zones.len().saturating_sub(1);
        for (k, (z, g)) in zones.iter().zip(icon_g).enumerate() {
            let color = if z.hovered() && k == last {
                ui.visuals().error_fg_color
            } else {
                fg
            };
            if z.hovered() && k != last {
                // Наведённый значок-кнопка — лёгкая подложка его цветом.
                painter.rect_filled(z.rect.shrink2(egui::vec2(1.0, 3.0)), 3.0, fg.gamma_multiply(0.18));
            }
            let pos = egui::pos2(
                z.rect.center().x - g.size().x / 2.0,
                rect.center().y - g.size().y / 2.0,
            );
            painter.galley(pos, g, color);
        }
    }
    (resp, zones)
}

/// Обычные цвета чипа: как у кнопки.
fn plain_chip_colors(ui: &egui::Ui, hovered: bool) -> (Color32, Color32, egui::Stroke) {
    let wv = if hovered {
        ui.visuals().widgets.hovered
    } else {
        ui.visuals().widgets.inactive
    };
    let stroke = if hovered { wv.bg_stroke } else { egui::Stroke::NONE };
    (wv.weak_bg_fill, wv.fg_stroke.color, stroke)
}

/// Переключатель канала с крестиком удаления внутри.
fn channel_chip(ui: &mut egui::Ui, name: &str, on: bool) -> Option<ChipAction> {
    let hint = if on {
        "Скрыть строки канала (ПКМ — показывать только его)"
    } else {
        "Показывать строки канала (ПКМ — показывать только его)"
    };
    let (resp, x_resp) = x_chip(ui, name, hint, &format!("Перестать слушать «{name}»"), |ui, hovered| {
        let (fill, _, stroke) = plain_chip_colors(ui, hovered);
        if on {
            let sel = ui.visuals().selection;
            (sel.bg_fill, sel.stroke.color, stroke)
        } else {
            (fill, ui.visuals().weak_text_color(), stroke)
        }
    });
    let mut only = false;
    resp.context_menu(|ui| {
        if ui.button("Показывать только этот канал").clicked() {
            only = true;
        }
    });
    if x_resp.clicked() {
        Some(ChipAction::Remove)
    } else if only {
        Some(ChipAction::Only)
    } else if resp.clicked() {
        Some(ChipAction::Toggle)
    } else {
        None
    }
}

/// Отступ между колонкой подписей и полями; посередине проходит вертикальная черта.
const LABEL_GAP: f32 = 16.0;

/// Строка «подпись | содержимое»: подпись прижата влево в колонке ширины `label_w`,
/// содержимое начинается сразу за чертой и переносится по ширине.
fn label_row_inner<R>(
    ui: &mut egui::Ui,
    label_w: f32,
    label: &str,
    hint: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.horizontal_top(|ui| {
        let h = ui.spacing().interact_size.y;
        ui.allocate_ui_with_layout(
            egui::vec2(label_w, h),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_size(egui::vec2(label_w, h));
                ui.label(label).on_hover_text(hint);
            },
        );
        ui.add_space(LABEL_GAP - ui.spacing().item_spacing.x);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), h),
            egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
            add,
        )
        .inner
    })
    .inner
}

fn labeled_row(
    ui: &mut egui::Ui,
    label_w: f32,
    label: &str,
    hint: &str,
    add: impl FnOnce(&mut egui::Ui) -> bool,
) -> bool {
    label_row_inner(ui, label_w, label, hint, add)
}

/// Enter в однострочном поле (оно при этом теряет фокус).
fn enter_pressed(ui: &egui::Ui, r: &egui::Response) -> bool {
    r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

/// Строка окна настроек: подпись слева, элемент управления у правого края.
fn setting_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(32.0);
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
    });
}

/// Ряд кнопок-сегментов в общей рамке; выбранный сегмент подсвечен.
fn segmented<T: Copy + PartialEq>(ui: &mut egui::Ui, value: &mut T, options: &[(T, RichText, &str)]) {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(6)
        .inner_margin(3)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                // В раскладке справа налево (у правого края окна) идём с конца, чтобы порядок сохранился.
                let rtl = ui.layout().main_dir() == egui::Direction::RightToLeft;
                let ordered: Vec<_> = if rtl {
                    options.iter().rev().collect()
                } else {
                    options.iter().collect()
                };
                for (v, icon, hint) in ordered {
                    let r = ui
                        .add(
                            egui::Button::selectable(*value == *v, icon.clone().size(18.0))
                                .min_size(egui::vec2(36.0, 24.0)),
                        )
                        .on_hover_text(*hint);
                    if r.clicked() {
                        *value = *v;
                    }
                }
            });
        });
}

/// Переключатель «вкл/выкл».
fn toggle_switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = ui.spacing().interact_size.y * egui::vec2(2.0, 1.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Switch, ui.is_enabled(), *on, "")
    });
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let v = ui.style().interact_selectable(&resp, *on);
        let radius = 0.5 * rect.height();
        ui.painter()
            .rect(rect, radius, v.bg_fill, v.bg_stroke, egui::StrokeKind::Inside);
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        ui.painter()
            .circle(egui::pos2(x, rect.center().y), 0.75 * radius, v.bg_fill, v.fg_stroke);
    }
    resp.on_hover_text(if *on { "Вкл." } else { "Выкл." })
}

fn status_look(s: &ConnStatus) -> (Color32, String) {
    match s {
        ConnStatus::Disconnected => (GRAY, "отключено".into()),
        ConnStatus::Connecting => (ORANGE, "подключение…".into()),
        ConnStatus::Connected => (GREEN, "подключено".into()),
        ConnStatus::Reconnecting(e) => (ORANGE, format!("переподключение: {e}")),
    }
}

/// Поле для новой подстроки (добавляется по Enter) и чипы уже применённых.
/// Возвращает true, если список изменился.
fn filter_items(ui: &mut egui::Ui, list: &mut Vec<String>, input: &mut String) -> bool {
    let mut changed = false;
    let r = ui.add(
        egui::TextEdit::singleline(input)
            .hint_text("подстрока")
            .desired_width(160.0),
    );
    if enter_pressed(ui, &r) && !input.is_empty() {
        let f = std::mem::take(input);
        if !list.contains(&f) {
            list.push(f);
            changed = true;
        }
        r.request_focus();
    }
    let mut i = 0;
    while i < list.len() {
        let (_, x) = x_chip(ui, &list[i], "", "Убрать", plain_chip_colors);
        if x.clicked() {
            list.remove(i);
            changed = true;
        } else {
            i += 1;
        }
    }
    if list.len() > 1 && ui.button("Сбросить").clicked() {
        list.clear();
        changed = true;
    }
    changed
}

/// Поле поиска, переходы ▲/▼, счётчик и закреплённые запросы. Ctrl+F ставит фокус в поле.
/// Enter закрепляет запрос из поля (у каждого свой цвет), в пустом поле — следующее совпадение.
fn search_items(
    ui: &mut egui::Ui,
    search: &mut Search,
    pal: LogPalette,
    rows: &[Row],
    visible: &[usize],
) -> bool {
    let id = egui::Id::unique("log_search");
    let r = ui.add(
        egui::TextEdit::singleline(&mut search.query)
            .id(id)
            .hint_text("Ctrl+F")
            .return_key(None)
            .desired_width(200.0),
    );
    if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
        r.request_focus();
    }
    // Цвет, которым подсвечивается текст из поля, — полоска под полем.
    if !search.query.is_empty() {
        let rect = r.rect;
        ui.painter().hline(
            rect.x_range().shrink(3.0),
            rect.bottom() - 1.5,
            egui::Stroke::new(3.0, pal.term_bg(search.terms.len())),
        );
    }
    if r.has_focus() {
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter)) {
            search.step(false);
        } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            let q = search.query.trim().to_string();
            if q.is_empty() {
                search.step(true);
            } else {
                search.query.clear();
                if !search.terms.iter().any(|t| t.to_lowercase() == q.to_lowercase()) {
                    search.terms.push(q);
                }
            }
        }
    }
    // Esc очищает поле, а в пустом поле — снимает все закреплённые запросы.
    // Esc снимает фокус с поля, поэтому проверяем и только что потерянный фокус.
    if (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        if search.query.is_empty() {
            search.terms.clear();
        } else {
            search.query.clear();
        }
    }
    let any = !search.matches.is_empty();
    if ui
        .add_enabled(any, egui::Button::new("⏶"))
        .on_hover_text("Предыдущее (Shift+Enter)")
        .clicked()
    {
        search.step(false);
    }
    if ui
        .add_enabled(any, egui::Button::new("⏷"))
        .on_hover_text("Следующее (Enter в пустом поле)")
        .clicked()
    {
        search.step(true);
    }
    if !search.query.is_empty() || !search.terms.is_empty() {
        let text = match search.current {
            Some(c) if any => format!("{} из {}", c + 1, search.matches.len()),
            _ => "нет совпадений".to_string(),
        };
        ui.label(RichText::new(text).color(GRAY));
    }
    let mut i = 0;
    while i < search.terms.len() {
        let bg = pal.term_bg(i);
        let icons = [
            ("⏶", "Предыдущее вхождение этого запроса"),
            ("⏷", "Следующее вхождение этого запроса"),
            ("✖", "Убрать запрос"),
        ];
        let (_, z) = icon_chip(ui, &search.terms[i], "", &icons, |ui, hovered| {
            let stroke = if hovered {
                ui.visuals().widgets.hovered.bg_stroke
            } else {
                egui::Stroke::NONE
            };
            (bg, Color32::BLACK, stroke)
        });
        if z[0].clicked() {
            search.step_term(rows, visible, i, false);
        }
        if z[1].clicked() {
            search.step_term(rows, visible, i, true);
        }
        if z[2].clicked() {
            search.terms.remove(i);
        } else {
            i += 1;
        }
    }
    if search.terms.len() > 1
        && ui
            .button("Сбросить")
            .on_hover_text("Убрать все запросы (Esc в пустом поле)")
            .clicked()
    {
        search.terms.clear();
    }
    false
}

/// Добавляет текст, подсвечивая вхождения поисковых запросов `patterns` (каждый своим цветом)
/// и выделенного текста `marked` (всё в нижнем регистре). Поиск перекрывает выделение.
fn append_highlighted(
    job: &mut LayoutJob,
    text: &str,
    patterns: &[String],
    marked: &str,
    fmt: TextFormat,
    pal: LogPalette,
) {
    let lower = text.to_lowercase();
    // Подсветка возможна, только если нижний регистр не меняет длину строки в байтах.
    if (patterns.is_empty() && marked.is_empty()) || lower.len() != text.len() {
        job.append(text, 0.0, fmt);
        return;
    }
    // Стиль каждого байта: 0 — обычный, 1 — выделенный текст, 2 + k — совпадение запроса k.
    let mut style = vec![0u8; text.len()];
    let all = std::iter::once((marked, 1u8))
        .chain(patterns.iter().enumerate().map(|(k, p)| (p.as_str(), (k + 2).min(255) as u8)));
    for (pattern, level) in all {
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
    let mut pos = 0;
    while pos < text.len() {
        let level = style[pos];
        let end = style[pos..]
            .iter()
            .position(|&l| l != level)
            .map_or(text.len(), |n| pos + n);
        let mut f = fmt.clone();
        if level > 0 {
            f.color = Color32::BLACK;
            f.background = if level == 1 {
                pal.mark_bg()
            } else {
                pal.term_bg(level as usize - 2)
            };
        }
        job.append(&text[pos..end], 0.0, f);
        pos = end;
    }
}

/// Текст строки лога: дата и время, канал (на вкладке «Все») и сообщение.
/// Длинные строки переносятся по ширине `wrap_width`.
#[allow(clippy::too_many_arguments)]
fn row_job(
    row: &Row,
    all_tab: bool,
    ch_width: usize,
    pal: LogPalette,
    patterns: &[String],
    marked: &str,
    wrap_width: f32,
) -> LayoutJob {
    let font = FontId::monospace(13.0);
    let fmt = |c: Color32| TextFormat::simple(font.clone(), c);
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.append(
        &row.entry.time.format("%d.%m %H:%M:%S%.3f  ").to_string(),
        0.0,
        fmt(pal.time()),
    );
    if all_tab {
        let ch: &str = &row.entry.channel;
        let (name, c) = if ch.is_empty() {
            ("*", pal.special())
        } else {
            (ch, pal.channel(ch))
        };
        job.append(&format!("{name:<ch_width$}  "), 0.0, fmt(c));
    }
    let c = if row.entry.channel.is_empty() {
        pal.special()
    } else {
        pal.text
    };
    append_highlighted(&mut job, &row.line, patterns, marked, fmt(c), pal);
    job
}

/// Досчитывает высоты строк (с учётом переноса) для текущих ширины и раскладки.
fn ensure_heights(
    heights: &mut Vec<f32>,
    heights_key: &mut Option<HeightsKey>,
    rows: &[Row],
    ui: &egui::Ui,
    key: HeightsKey,
    pal: LogPalette,
) {
    if *heights_key != Some(key) {
        heights.clear();
        *heights_key = Some(key);
    }
    if heights.len() >= rows.len() {
        return;
    }
    let width = f32::from_bits(key.width_bits);
    heights.reserve(rows.len() - heights.len());
    let mut i = heights.len();
    while i < rows.len() {
        let job = row_job(
            &rows[i],
            key.all_tab,
            key.ch_width,
            pal,
            &[],
            "",
            width,
        );
        heights.push(ui.fonts_mut(|fonts| fonts.layout_job(job)).size().y);
        i += 1;
    }
}

fn status_dot(ui: &mut egui::Ui, color: Color32) -> egui::Response {
    let size = egui::vec2(10.0, ui.spacing().interact_size.y);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, color);
    resp
}

fn format_entry(e: &LogEntry) -> String {
    let ch: &str = if e.channel.is_empty() { "*" } else { &e.channel };
    format!("{} {} {}", e.time.format("%Y-%m-%d %H:%M:%S%.3f"), ch, e.text)
}

/// 20000 → «20 000».
fn fmt_count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    fn time(secs: i64) -> chrono::NaiveDateTime {
        chrono::DateTime::from_timestamp(secs, 0)
            .expect("valid timestamp")
            .naive_utc()
    }

    fn entry(secs: i64, channel: &str, text: &str) -> LogEntry {
        LogEntry {
            time: time(secs),
            channel: Arc::from(channel),
            text: text.to_string(),
        }
    }

    #[test]
    fn step_term_walks_only_its_term() {
        let mut v = View::default();
        for (k, t) in ["a x", "b", "a", "x", "a b"].iter().enumerate() {
            v.push(entry(k as i64, "ch", t));
        }
        let empty: Vec<String> = Vec::new();
        v.visible(&empty, &empty, true);
        let key = (v.channels_version, v.generation, v.filter_version);
        v.search.terms = vec!["a".into(), "b".into()];
        v.search.update(&v.rows, &v.cache, key);
        // Совпадения: строки 0, 1, 2, 4; текущее — последнее (строка 4).
        assert_eq!(v.search.matches, vec![0, 1, 2, 4]);
        let pos = |v: &View| v.search.current.map(|c| v.search.matches[c]);
        v.search.step_term(&v.rows, &v.cache, 0, false);
        assert_eq!(pos(&v), Some(2));
        v.search.step_term(&v.rows, &v.cache, 0, false);
        assert_eq!(pos(&v), Some(0));
        v.search.step_term(&v.rows, &v.cache, 0, false);
        assert_eq!(pos(&v), Some(4), "по кругу к последнему");
        v.search.step_term(&v.rows, &v.cache, 1, true);
        assert_eq!(pos(&v), Some(1), "после последнего — к первому вхождению «b»");
    }

    #[test]
    fn hidden_channels_filter_rows() {
        let mut v = View::default();
        v.push(entry(0, "a", "one"));
        v.push(entry(1, "b", "two"));
        v.push(entry(2, "", "service"));
        let empty: Vec<String> = Vec::new();
        assert_eq!(v.visible(&empty, &empty, true).len(), 3);
        v.set_hidden("a", true);
        assert_eq!(v.visible(&empty, &empty, true), &[1, 2]);
        let channels = vec!["a".to_string(), "b".to_string()];
        v.show_only(&channels, "a");
        assert_eq!(v.visible(&empty, &empty, true), &[0, 2]);
        v.retain_hidden(&["a".to_string()]);
        assert_eq!(v.visible(&empty, &empty, true).len(), 3);
        v.set_hidden("b", true);
        v.show_all_channels();
        assert_eq!(v.visible(&empty, &empty, true).len(), 3);
    }

    /// Щелчок по чипу в точке `at` (доля ширины чипа); возвращает действие.
    fn click_chip(at: f32) -> Option<&'static str> {
        let ctx = egui::Context::default();
        let mut rect = egui::Rect::NOTHING;
        let frame = |events: Vec<egui::Event>, rect: &mut egui::Rect| {
            let mut result = None;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                let before = ui.cursor().min;
                result = channel_chip(ui, "channel", true).map(|a| match a {
                    ChipAction::Toggle => "toggle",
                    ChipAction::Only => "only",
                    ChipAction::Remove => "remove",
                });
                *rect = egui::Rect::from_min_max(before, ui.min_rect().max);
            });
            out.textures_delta.clear();
            result
        };
        frame(Vec::new(), &mut rect);
        let pos = egui::pos2(rect.left() + rect.width() * at, rect.center().y);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(pos)], &mut rect);
        frame(vec![button(true)], &mut rect);
        frame(vec![button(false)], &mut rect)
    }

    #[test]
    fn channel_chip_click_zones() {
        assert_eq!(click_chip(0.2), Some("toggle"));
        assert_eq!(click_chip(0.95), Some("remove"));
    }

    #[test]
    fn row_keeps_newlines() {
        let r = Row::new(entry(0, "", "a\r\nb\nc"));
        assert_eq!(r.line, "a\nb\nc");
        assert_eq!(r.lower, "a\r\nb\nc");
    }

    #[test]
    fn push_keeps_only_last_rows() {
        let mut v = View::default();
        for i in 0..MAX_ROWS as i64 + 5 {
            v.push(entry(i, "", "x"));
        }
        assert_eq!(v.rows.len(), MAX_ROWS);
        assert_eq!(v.rows[0].entry.time, time(5));
        assert_eq!(v.rows[MAX_ROWS - 1].entry.time, time(MAX_ROWS as i64 + 4));
    }

    #[test]
    fn merge_channel_skips_duplicates() {
        let mut v = View::default();
        v.push(entry(0, "a", "one"));
        v.push(entry(2, "b", "other"));
        let history = vec![entry(0, "a", "one"), entry(1, "a", "two")];
        v.merge_channel("a", history.clone());
        v.merge_channel("a", history);
        let texts: Vec<&str> = v.rows.iter().map(|r| r.line.as_str()).collect();
        assert_eq!(texts, ["one", "two", "other"]);
    }

    #[test]
    fn long_line_wraps_and_newlines_add_rows() {
        let ctx = egui::Context::default();
        let long = Row::new(entry(0, "ch", &"word ".repeat(80)));
        let multi = Row::new(entry(0, "ch", "one\ntwo\nthree"));
        let (mut long_h, mut multi_h, mut row_h) = (0.0f32, 0.0f32, 0.0f32);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let width = 160.0;
            let layout = |r: &Row| {
                let job = row_job(r, false, 6, LogPalette::new(&egui::Visuals::dark(), false), &[], "", width);
                ui.fonts_mut(|f| f.layout_job(job)).size().y
            };
            long_h = layout(&long);
            multi_h = layout(&multi);
            row_h = ui.fonts_mut(|f| f.row_height(&FontId::monospace(13.0)));
        });
        // Кадр не выводится на экран: дельты текстур нужно «погасить», иначе egui паникует.
        out.textures_delta.clear();
        assert!(row_h > 0.0);
        assert!(
            long_h > row_h * 2.0,
            "длинная строка должна переноситься: {long_h} при высоте строки {row_h}"
        );
        assert!(
            multi_h >= row_h * 3.0 - 2.0,
            "переводы строк должны давать несколько строк: {multi_h} при высоте строки {row_h}"
        );
    }

    #[test]
    fn keeps_rows_across_midnight() {
        // Смена суток не должна очищать окно: строки живут до вытеснения по лимиту.
        let mut v = View::default();
        v.push(entry(0, "ch", "до полуночи"));
        v.push(entry(86_400, "ch", "после полуночи"));
        assert_eq!(v.rows.len(), 2);
    }

    #[test]
    fn trim_keeps_visible_and_search_positions_valid() {
        let mut v = View::default();
        for i in 0..MAX_ROWS as i64 {
            let text = if i % 1000 == 0 { "match" } else { "noise" };
            v.push(entry(i, "ch", text));
        }
        let empty: Vec<String> = Vec::new();
        v.visible(&empty, &empty, true);
        let key = (v.channels_version, v.generation, v.filter_version);
        v.search.query = "match".to_string();
        v.search.update(&v.rows, &v.cache, key);
        assert!(v.search.matches.len() > 1);

        for i in 0..3 {
            let text = if i == 1 { "match" } else { "noise" };
            v.push(entry(MAX_ROWS as i64 + i, "ch", text));
        }
        v.visible(&empty, &empty, true);
        assert_eq!(v.rows.len(), MAX_ROWS);
        for &i in &v.cache {
            assert!(i < v.rows.len(), "кэш ссылается на вытесненную строку");
        }
        for &pos in &v.search.matches {
            assert!(
                v.rows[v.cache[pos]].entry.text.contains("match"),
                "позиция совпадения съехала после вытеснения строк"
            );
        }
        if let Some(c) = v.search.current {
            let pos = v.search.matches[c];
            assert!(v.rows[v.cache[pos]].entry.text.contains("match"));
        }
    }
}

