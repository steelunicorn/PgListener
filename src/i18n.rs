//! Локализация интерфейса: русский и английский.
//!
//! Все пользовательские строки собраны в одном списке `strings!`: вариант [`Key`],
//! русский текст и английский перевод. Форматируемые строки хранят плейсхолдеры
//! (`{}`, `{e}`, `{ch}` и т. п.); [`tf`] подставляет аргументы по порядку, поэтому
//! шаблоны обоих языков должны иметь одинаковое число плейсхолдеров — это проверяет тест.
//!
//! Язык хранится в глобальной ячейке [`set_current`]: её читают и окно, и фоновые задачи
//! (воркер подключения, мост), когда формируют очередное сообщение.

use std::sync::atomic::{AtomicU8, Ordering};

/// Язык интерфейса.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Ru,
    En,
}

impl Lang {
    /// Код для хранения в настройках.
    pub fn code(self) -> &'static str {
        match self {
            Lang::Ru => "ru",
            Lang::En => "en",
        }
    }

    /// Разбирает код из настроек или переменной окружения.
    pub fn from_code(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ru" | "russian" => Some(Lang::Ru),
            "en" | "english" => Some(Lang::En),
            _ => None,
        }
    }

    /// Подпись на переключателе RU|EN.
    pub fn label(self) -> &'static str {
        match self {
            Lang::Ru => "RU",
            Lang::En => "EN",
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Язык, на котором сейчас говорит интерфейс и фоновые задачи.
pub fn current() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == 1 {
        Lang::En
    } else {
        Lang::Ru
    }
}

/// Переключает язык для всего процесса.
pub fn set_current(lang: Lang) {
    CURRENT.store(
        match lang {
            Lang::Ru => 0,
            Lang::En => 1,
        },
        Ordering::Relaxed,
    );
}

macro_rules! strings {
    ($($name:ident => $ru:literal => $en:literal,)*) => {
        /// Ключ пользовательской строки.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Key {
            $($name,)*
        }

        impl Key {
            /// Русский текст (шаблон с плейсхолдерами).
            pub fn ru(self) -> &'static str {
                match self {
                    $(Key::$name => $ru,)*
                }
            }

            /// Английский текст (шаблон с плейсхолдерами).
            pub fn en(self) -> &'static str {
                match self {
                    $(Key::$name => $en,)*
                }
            }

            /// Текст на указанном языке.
            pub fn text(self, lang: Lang) -> &'static str {
                match lang {
                    Lang::Ru => self.ru(),
                    Lang::En => self.en(),
                }
            }
        }

        /// Все ключи — для проверки полноты в тестах.
        #[cfg(test)]
        pub const ALL: &[Key] = &[$(Key::$name,)*];
    };
}

strings! {
    // Общие подписи.
    All => "Все" => "All",
    Name => "Имя" => "Name",
    Address => "Адрес" => "Address",
    Host => "Хост" => "Host",
    Port => "Порт" => "Port",
    Database => "База данных" => "Database",
    User => "Пользователь" => "User",
    Password => "Пароль" => "Password",
    Authentication => "Аутентификация" => "Authentication",
    Settings => "Настройки" => "Settings",
    Cancel => "Отмена" => "Cancel",
    Save => "Сохранить" => "Save",
    Open => "Открыть" => "Open",
    Delete => "Удалить" => "Delete",
    Rename => "Переименовать…" => "Rename…",
    Duplicate => "Дублировать…" => "Duplicate…",
    DupSuffix => "{} (копия)" => "{} (copy)",
    Browse => "Обзор…" => "Browse…",
    Reset => "Сбросить" => "Reset",
    Retry => "Повтор" => "Retry",
    Show => "Показать" => "Show",
    Quit => "Выход" => "Quit",
    On => "Вкл." => "On",
    Off => "Выкл." => "Off",
    Yes => "Да, подключить" => "Yes, connect",
    No => "Нет" => "No",
    ErrorTitle => "Ошибка" => "Error",
    Unknown => "неизвестно" => "unknown",
    Timeout => "таймаут" => "timeout",

    // Шапка, кнопки, дерево подключений.
    ConnectBtn => "▶ Подключить" => "▶ Connect",
    DisconnectBtn => "⏹ Отключить" => "⏹ Disconnect",
    Connect => "Подключить" => "Connect",
    Disconnect => "Отключить" => "Disconnect",
    PropertiesBtn => "⚙ Свойства" => "⚙ Properties",
    PropertiesMenu => "Свойства…" => "Properties…",
    ConnectionProperties => "Свойства подключения" => "Connection properties",
    NewConnectionBtn => "➕ Подключение" => "➕ Connection",
    NewConnection => "Новое подключение" => "New connection",
    NewConnectionMenu => "Новое подключение…" => "New connection…",
    FolderBtn => "📁 Папка" => "📁 Folder",
    NewFolder => "Новая папка" => "New folder",
    NewFolderMenu => "Новая папка…" => "New folder…",
    FolderNameHint => "имя папки" => "folder name",
    LogFolderBtn => "📂 Папка логов" => "📂 Log folder",
    ClearWindowBtn => "🗑 Очистить окно" => "🗑 Clear window",
    ClearWindowHint => "Очищает только окно; файлы логов не трогаются" => "Clears the window only; log files are untouched",
    EmptyStateHint => "Создайте подключение кнопкой «➕ Подключение» слева" => "Create a connection with the “➕ Connection” button on the left",
    RootFolder => "(корень)" => "(root)",
    RenameFolder => "Переименовать папку" => "Rename folder",
    DeleteFolder => "Удалить папку" => "Delete folder",
    DeleteFolderConfirm => "Удалить папку «{}»? Её содержимое переместится на уровень выше." => "Delete folder “{}”? Its contents will move one level up.",
    DeleteConnectionConfirm => "Удалить подключение «{}»? Файлы логов останутся на диске." => "Delete connection “{}”? Log files will remain on disk.",
    MoveTo => "Переместить в" => "Move to",

    // Вкладка подключения: каналы и параметры.
    ChannelsHint => "Прослушиваемые каналы (изменения применяются сразу после сохранения, без переподключения):" => "Listened channels (changes apply right after saving, without reconnecting):",
    NewChannelHint => "новый канал" => "new channel",
    NewChannelEnter => "новый канал, Enter" => "new channel, Enter",
    RemoveChannel => "Убрать канал" => "Remove channel",
    RemoveChip => "Убрать" => "Remove",
    ShowOnlyChannel => "Показывать только этот канал" => "Show only this channel",
    ShowAllChannels => "Показывать все каналы" => "Show all channels",
    ChannelShowHint => "Показывать строки канала (ПКМ — показывать только его)" => "Show channel lines (right-click to show only this one)",
    ChannelHideHint => "Скрыть строки канала (ПКМ — показывать только его)" => "Hide channel lines (right-click to show only this one)",
    StopListening => "Перестать слушать «{name}»" => "Stop listening to “{name}”",
    LogDir => "Каталог логов" => "Log folder",
    SslDisable => "Выключен" => "Disabled",
    SslPrefer => "Предпочтительно" => "Preferred",
    SslRequire => "Обязательно" => "Required",
    SshToggle => "Подключаться через SSH-туннель" => "Connect via SSH tunnel",
    SshHost => "SSH-хост" => "SSH host",
    SshPort => "SSH-порт" => "SSH port",
    SshUser => "SSH-пользователь" => "SSH user",
    SshPassword => "SSH-пароль" => "SSH password",
    SshHostHint => "При SSH-туннеле — адрес сервера БД со стороны SSH-сервера (обычно localhost)" => "With an SSH tunnel — the DB server address as seen from the SSH server (usually localhost)",
    KeyFile => "Файл ключа" => "Key file",
    KeyFileHint => "OpenSSH/PEM, путь можно относительно exe" => "OpenSSH/PEM, the path may be relative to the exe",
    KeyPassword => "Пароль ключа" => "Key password",
    MasterPassword => "Мастер-пароль" => "Master password",
    NewPassword => "Новый пароль" => "New password",
    KeyField => "Ключ" => "Key",
    Set => "Задать" => "Set",
    Change => "Сменить" => "Change",
    RemovePassword => "Снять пароль" => "Remove password",
    ErasePasswords => "Стереть пароли" => "Erase passwords",
    ForgotPassword => "Забыл пароль…" => "Forgot password…",
    RememberOnPC => "Запомнить на этом компьютере" => "Remember on this computer",
    RememberHint => "Пароль не будет спрашиваться при запуске под вашей учётной записью Windows. На другом компьютере его придётся ввести." => "The password will not be asked at startup under your Windows account. On another computer it will have to be entered.",
    ApplyRemember => "Применить «запомнить»" => "Apply “remember”",
    PasswordsMismatch => "Пароли не совпадают" => "Passwords do not match",
    WrongPassword => "Неверный пароль" => "Wrong password",
    EnterPassword => "Введите пароль" => "Enter password",
    EnterMasterPassword => "Введите мастер-пароль" => "Enter master password",
    PasswordsEncrypted => "Пароли зашифрованы мастер-паролем" => "Passwords are encrypted with the master password",
    PasswordsPlainSetMaster => "Пароли хранятся открытым текстом. Задать мастер-пароль" => "Passwords are stored in plain text. Set a master password",
    PasswordsPlainInfo => "Сейчас пароли подключений хранятся в pglistener.db открытым текстом." => "Connection passwords are currently stored in pglistener.db in plain text.",
    StoreEncryptedInfo => "Пароли подключений в pglistener.db зашифрованы." => "Connection passwords in pglistener.db are encrypted.",
    StoreEncryptedScheme => "Пароли подключений зашифрованы (AES-256-GCM, ключ из мастер-пароля через Argon2)." => "Connection passwords are encrypted (AES-256-GCM, key derived from the master password via Argon2).",
    PasswordsPlainAgain => "Пароли снова будут храниться открытым текстом" => "Passwords will be stored in plain text again",
    ClearPasswordsConfirm => "Все сохранённые пароли подключений будут стёрты, остальные настройки сохранятся. Продолжить?" => "All saved connection passwords will be erased; other settings are kept. Continue?",
    IfKeyEncrypted => "если ключ зашифрован" => "if the key is encrypted",
    TestConnection => "Проверить подключение" => "Test connection",
    TestingConnection => "Проверка подключения…" => "Testing connection…",

    // Мост.
    BridgeToggle => "Мост: пускать к БД клиентов из локальной сети" => "Bridge: let clients on the local network reach the DB",
    BridgeListenHint => "0.0.0.0 — все сетевые интерфейсы, 127.0.0.1 — только этот ПК" => "0.0.0.0 — all network interfaces, 127.0.0.1 — this PC only",
    BridgeAllowHint => "Адреса и подсети через запятую, например: 192.168.1.0/24, 10.0.0.5" => "Addresses and subnets separated by commas, e.g. 192.168.1.0/24, 10.0.0.5",
    BridgeHint => "PgListener принимает подключения на указанном порту и пробрасывает их к БД (через SSH-туннель, если он включён). Клиент указывает адрес этого ПК и порт моста, а логин и пароль к БД вводит свой. Мост работает, пока подключение активно." => "PgListener accepts connections on the given port and forwards them to the DB (via the SSH tunnel when enabled). The client uses this PC's address and the bridge port, and enters its own DB login and password. The bridge lives as long as the connection is active.",
    AllowedAddresses => "Разрешённые адреса" => "Allowed addresses",
    EmptyAllAddresses => "пусто — все адреса" => "empty — all addresses",
    AllAddresses => "все адреса" => "all addresses",
    BadNet => "неверный адрес или подсеть: {s}" => "invalid address or subnet: {s}",
    BridgeNotStarted => "{e}, мост не запущен" => "{e}, bridge not started",
    BridgeBindFailed => "не удалось открыть {addr}: {e}. Повтор через {} с" => "failed to open {addr}: {e}. Retry in {} s",
    BridgeListening => "принимает подключения на {addr}, разрешены: {}" => "listening on {addr}, allowed: {}",
    BridgeAcceptError => "ошибка приёма подключения: {e}" => "accept error: {e}",
    PeerRejected => "отклонено подключение с {peer}: адрес не в списке разрешённых" => "connection from {peer} rejected: address not in the allow list",
    PeerConnected => "подключение с {peer}" => "connection from {peer}",
    PeerDisconnected => "{peer} отключён" => "{peer} disconnected",
    PeerDisconnectedBytes => "{peer} отключён, передано к БД {}, от БД {}" => "{peer} disconnected, transferred to DB {}, from DB {}",
    PeerDisconnectedErr => "{peer} отключён: {e:#}" => "{peer} disconnected: {e:#}",
    BridgeMsg => "Мост: {t}" => "Bridge: {t}",
    BridgeMsgText => "Мост: {text}" => "Bridge: {text}",
    BridgeMsgErr => "Мост: {e}" => "Bridge: {e}",
    BridgeConn => "· мост {}:{}" => "· bridge {}:{}",
    UnitB => "{n} Б" => "{n} B",
    UnitKb => "{:.1} КБ" => "{:.1} KB",
    UnitMb => "{:.1} МБ" => "{:.1} MB",

    // Окно лога: фильтры, поиск, счётчик, чип слежения.
    IncludeLabel => "Показывать вхождения" => "Include",
    HideLabel => "Скрывать вхождения" => "Hide",
    SearchLabel => "Поиск" => "Search",
    SubstringHint => "подстрока" => "substring",
    IncludesHint => "Если задано, показываются только строки, содержащие любую из подстрок. Регистр не учитывается." => "If set, only lines containing any of the substrings are shown. Case is ignored.",
    ExcludesHint => "Строки, содержащие любую из подстрок, не показываются. Регистр не учитывается." => "Lines containing any of the substrings are hidden. Case is ignored.",
    SearchHint => "Enter закрепляет запрос, у каждого запроса свой цвет. Enter в пустом поле — следующее совпадение." => "Enter pins a query; each query has its own colour. Enter in the empty field — next match.",
    PrevSearchHint => "Предыдущее (Shift+Enter)" => "Previous (Shift+Enter)",
    NextSearchHint => "Следующее (Enter в пустом поле)" => "Next (Enter in the empty field)",
    PrevMatch => "Предыдущее вхождение этого запроса" => "Previous match of this query",
    NextMatch => "Следующее вхождение этого запроса" => "Next match of this query",
    RemoveQuery => "Убрать запрос" => "Remove query",
    ClearAllQueries => "Убрать все запросы (Esc в пустом поле)" => "Clear all queries (Esc in the empty field)",
    NoMatches => "нет совпадений" => "no matches",
    MatchOf => "{} из {}" => "{} of {}",
    Counter => "Строк в окне: {}, показано: {}" => "Rows in window: {}, shown: {}",
    CounterLimited => "Строк в окне: {} (хранятся последние), показано: {}" => "Rows in window: {} (last ones kept), shown: {}",
    CopyLine => "Копировать строку" => "Copy line",
    CopyAllShown => "Копировать все показанные строки" => "Copy all shown lines",
    TailFollowHintOn => "Слежение включено: окно держится в конце. Щёлкните, чтобы остаться здесь" => "Following is on: the window stays at the end. Click to stay here",
    TailFollowHintOff => "Перейти в конец лога и следить за новыми строками" => "Jump to the end of the log and follow new lines",
    SystemMessages => "Системные сообщения" => "System messages",
    SystemMessagesHint => "Сообщения самого PgListener (подключение, ошибки) в окне лога" => "PgListener's own messages (connection, errors) in the log window",
    MinimizeToTray => "Сворачивать в трей" => "Minimize to tray",
    SystemTheme => "Как в системе" => "System",
    LightTheme => "Светлая" => "Light",
    DarkTheme => "Тёмная" => "Dark",
    HackerTheme => "Матрица" => "Matrix",
    SoftwareRenderWarning => "Видеокарта не используется: окно рисует процессор, это заметно его нагружает." => "The GPU is not used: the window is rendered by the CPU, which noticeably loads it.",
    RendererHint => "OpenGL используется, если он есть; иначе DirectX 12. Принудительно: переменная окружения PGLISTENER_RENDERER=opengl или dx12." => "OpenGL is used when available; otherwise DirectX 12. Force it with the PGLISTENER_RENDERER=opengl or dx12 environment variable.",

    // Статусы и системные сообщения подключения.
    Connecting => "подключение…" => "connecting…",
    Connected => "подключено" => "connected",
    Reconnecting => "переподключение: {e}" => "reconnecting: {e}",
    Disconnected => "отключено" => "disconnected",
    DisconnectedMsg => "Отключено" => "Disconnected",
    ConnectedTo => "Подключено к {}:{}/{}{}. Каналы: {}" => "Connected to {}:{}/{}{}. Channels: {}",
    SshVia => " через SSH {}" => " via SSH {}",
    NoneChannels => "(нет)" => "(none)",
    ReconnectError => "Ошибка: {reason}. Повторное подключение через {delay} с" => "Error: {reason}. Reconnecting in {delay} s",
    ChannelAdded => "Добавлен канал {ch}" => "Channel added: {ch}",
    ChannelRemoved => "Удалён канал {ch}" => "Channel removed: {ch}",
    RestoreConnections => "Восстановить подключения?" => "Restore connections?",
    RestoreActiveList => "При прошлом закрытии были активны:" => "Active at last shutdown:",
    ConnectionClosedByServer => "соединение закрыто сервером" => "connection closed by server",
    ServerNotResponding => "сервер не отвечает" => "server is not responding",
    PgConnectTimeout => "таймаут подключения к PostgreSQL" => "PostgreSQL connection timeout",
    SshConnectTimeout => "таймаут подключения к SSH-серверу" => "SSH server connection timeout",
    ConnectingTo => "подключение к {}:{}" => "connecting to {}:{}",
    ConnectTimeoutAddr => "таймаут подключения к {}:{}" => "connection timeout to {}:{}",
    ConnectingSsh => "подключение к SSH {}:{}" => "connecting to SSH {}:{}",
    SshTunnelTo => "SSH-туннель до {}:{}" => "SSH tunnel to {}:{}",
    ReadingKey => "чтение ключа {}" => "reading key {}",
    SshAuthFailed => "SSH: аутентификация не удалась" => "SSH: authentication failed",
    OpenGlUnavailable => "OpenGL недоступен ({e}), переключаюсь на DirectX 12" => "OpenGL is unavailable ({e}), switching to DirectX 12",

    // Ошибки хранилища и шифрования.
    DbOpenFailed => "Не удалось открыть pglistener.db: {e:#}" => "Failed to open pglistener.db: {e:#}",
    DbReadError => "Ошибка чтения базы: {e:#}" => "Database read error: {e:#}",
    DbLocked => "база заблокирована" => "database is locked",
    NoCryptoParams => "в базе нет параметров шифрования" => "no encryption parameters in the database",
    WrongKey => "неверный ключ" => "wrong key",
    CorruptedValue => "повреждённое значение" => "corrupted value",
    DpapiProtectFailed => "DPAPI: не удалось защитить ключ" => "DPAPI: failed to protect the key",
    SystemRng => "системный генератор случайных чисел" => "system random number generator",
}

/// Переводит ключ на текущий язык.
pub fn t(key: Key) -> &'static str {
    key.text(current())
}

/// Переводит ключ и подставляет аргументы вместо плейсхолдеров по порядку.
///
/// Аргументы принимаются как `&str`, `&String` и любые другие `AsRef<str>`.
pub fn tf<S: AsRef<str>>(key: Key, args: &[S]) -> String {
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    fill(key.text(current()), &args)
}

/// Подставляет аргументы в шаблон по порядку вхождения плейсхолдеров.
///
/// Плейсхолдером считается любая пара фигурных скобок (`{}`, `{e}`, `{:.1}`);
/// лишние аргументы игнорируются, недостающие оставляют плейсхолдер как есть.
pub fn fill(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    let mut next = 0;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|i| open + i) else {
            break;
        };
        out.push_str(&rest[..open]);
        if let Some(arg) = args.get(next) {
            out.push_str(arg);
            next += 1;
        } else {
            out.push_str(&rest[open..=close]);
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Язык по умолчанию: переменная окружения `PGLISTENER_LANG`, затем локаль ОС, иначе EN.
pub fn default_lang() -> Lang {
    if let Ok(v) = std::env::var("PGLISTENER_LANG") {
        if let Some(lang) = Lang::from_code(&v) {
            return lang;
        }
    }
    os_lang().unwrap_or(Lang::En)
}

/// Язык из локали операционной системы: русская локаль — RU, любая другая известная — EN.
#[cfg(windows)]
fn os_lang() -> Option<Lang> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    // SAFETY: функция без параметров, всегда возвращает LANGID текущего пользователя.
    let id = unsafe { GetUserDefaultUILanguage() } & 0x3ff;
    match id {
        // LANG_RUSSIAN.
        25 => Some(Lang::Ru),
        0 => None,
        _ => Some(Lang::En),
    }
}

#[cfg(not(windows))]
fn os_lang() -> Option<Lang> {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        let Ok(value) = std::env::var(var) else {
            continue;
        };
        let value = value.to_ascii_lowercase();
        if value.starts_with("ru") {
            return Some(Lang::Ru);
        }
        if !value.is_empty() {
            return Some(Lang::En);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_string_has_both_languages() {
        for &k in ALL {
            assert!(!k.ru().trim().is_empty(), "пустой русский текст: {k:?}");
            assert!(!k.en().trim().is_empty(), "пустой английский текст: {k:?}");
        }
    }

    #[test]
    fn placeholders_match_between_languages() {
        let count = |s: &str| s.matches('{').count();
        for &k in ALL {
            assert_eq!(
                count(k.ru()),
                count(k.en()),
                "разное число плейсхолдеров у {k:?}: {:?} / {:?}",
                k.ru(),
                k.en()
            );
        }
    }

    #[test]
    fn fill_substitutes_in_order() {
        let s = fill("a {x} b {} c {:.1}", &["1", "2", "3.5"]);
        assert_eq!(s, "a 1 b 2 c 3.5");
        assert_eq!(fill("без аргументов", &[]), "без аргументов");
        assert_eq!(fill("{} {}", &["x"]), "x {}");
    }

    #[test]
    fn lang_codes_roundtrip() {
        for lang in [Lang::Ru, Lang::En] {
            assert_eq!(Lang::from_code(lang.code()), Some(lang));
        }
        assert_eq!(Lang::from_code("EN"), Some(Lang::En));
        assert_eq!(Lang::from_code("de"), None);
    }
}
