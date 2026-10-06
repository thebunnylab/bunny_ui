//! The framework's own words — the few it puts on screen itself, in
//! the person's language.
//!
//! The mac's app menu and Window menu, the Edit items every platform
//! lists, the iPad's Settings row, a notification's default action on
//! a Linux desktop, the Android channel's name: nineteen words no app
//! can reach, because the shell writes them. They come from a
//! [`Catalog`] of sixteen languages, picked by the locale in effect —
//! and the app has the last word through [`install`], a hook that may
//! answer for any word in any language, or stay silent and leave the
//! table to speak.
//!
//! A word that names the app is a TEMPLATE with `{app}` where the name
//! goes, because the languages disagree on where that is: "Quit {app}",
//! "{app} beenden", "{app}を終了". [`Words::titled`] fills it.
//!
//! The tables follow each platform's own glossary where the platform
//! has one (the mac's menus are the mac's words), sentence case where
//! the language writes menus that way. A table is a plain `const` a
//! reviewer can read line by line; every table holds every word.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use motor::state::Locale;

use crate::catalog::{Catalog, Key, Strings, Table, fill};

/// How many words the framework owns — the length of every table.
pub const WORDS: usize = 19;

/// One of the framework's own words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Word {
    /// The app menu's first row: "About {app}".
    About,
    /// The app's settings, as the mac and the iPad file them: "Settings…".
    Settings,
    /// The Services submenu AppKit fills.
    Services,
    /// "Hide {app}".
    Hide,
    /// "Hide Others".
    HideOthers,
    /// "Show All".
    ShowAll,
    /// "Quit {app}".
    Quit,
    /// The Window menu's title.
    Window,
    /// The Window menu: "Minimize".
    Minimize,
    /// The Window menu: "Zoom".
    Zoom,
    /// The Window menu: "Bring All to Front".
    BringAllToFront,
    /// The standard edits, in every platform's order.
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    /// A notification's default action — the notification itself,
    /// clicked — where the desktop labels it.
    Open,
    /// The name of the one channel the app's notifications ride, where
    /// the platform lists channels.
    Notifications,
}

impl Word {
    /// Every word, in table order.
    pub const ALL: [Word; WORDS] = [
        Word::About,
        Word::Settings,
        Word::Services,
        Word::Hide,
        Word::HideOthers,
        Word::ShowAll,
        Word::Quit,
        Word::Window,
        Word::Minimize,
        Word::Zoom,
        Word::BringAllToFront,
        Word::Undo,
        Word::Redo,
        Word::Cut,
        Word::Copy,
        Word::Paste,
        Word::SelectAll,
        Word::Open,
        Word::Notifications,
    ];

    /// Does the word name the app — is its entry a template with `{app}`?
    pub const fn names_the_app(self) -> bool {
        matches!(self, Word::About | Word::Hide | Word::Quit)
    }
}

impl Key for Word {
    const COUNT: usize = WORDS;

    fn index(self) -> usize {
        self as usize
    }
}

/// One language's words.
pub type WordTable = Table<WORDS>;

// Every table lists the words in this order, on these four lines:
//   About, Settings, Services, Hide, HideOthers, ShowAll, Quit
//   Window, Minimize, Zoom, BringAllToFront
//   Undo, Redo, Cut, Copy, Paste, SelectAll
//   Open, Notifications
#[rustfmt::skip]
const EN: WordTable = Table { tag: "en", entries: [
    "About {app}", "Settings…", "Services", "Hide {app}", "Hide Others", "Show All", "Quit {app}",
    "Window", "Minimize", "Zoom", "Bring All to Front",
    "Undo", "Redo", "Cut", "Copy", "Paste", "Select All",
    "Open", "Notifications",
]};

/// Brazilian Portuguese, tagged `pt`: `pt-BR` and bare `pt` both land
/// here by truncation, while `pt-PT` has a table of its own.
#[rustfmt::skip]
const PT: WordTable = Table { tag: "pt", entries: [
    "Sobre o {app}", "Ajustes…", "Serviços", "Ocultar {app}", "Ocultar Outros", "Mostrar Tudo", "Encerrar {app}",
    "Janela", "Minimizar", "Zoom", "Trazer Tudo para a Frente",
    "Desfazer", "Refazer", "Recortar", "Copiar", "Colar", "Selecionar Tudo",
    "Abrir", "Notificações",
]};

#[rustfmt::skip]
const PT_PT: WordTable = Table { tag: "pt-PT", entries: [
    "Acerca de {app}", "Definições…", "Serviços", "Ocultar {app}", "Ocultar Outras", "Mostrar Tudo", "Sair de {app}",
    "Janela", "Minimizar", "Zoom", "Trazer Tudo para a Frente",
    "Anular", "Refazer", "Cortar", "Copiar", "Colar", "Selecionar Tudo",
    "Abrir", "Notificações",
]};

#[rustfmt::skip]
const ES: WordTable = Table { tag: "es", entries: [
    "Acerca de {app}", "Ajustes…", "Servicios", "Ocultar {app}", "Ocultar otros", "Mostrar todo", "Salir de {app}",
    "Ventana", "Minimizar", "Zoom", "Traer todo al frente",
    "Deshacer", "Rehacer", "Cortar", "Copiar", "Pegar", "Seleccionar todo",
    "Abrir", "Notificaciones",
]};

#[rustfmt::skip]
const FR: WordTable = Table { tag: "fr", entries: [
    "À propos de {app}", "Réglages…", "Services", "Masquer {app}", "Masquer les autres", "Tout afficher", "Quitter {app}",
    "Fenêtre", "Placer dans le Dock", "Zoom", "Tout ramener au premier plan",
    "Annuler", "Rétablir", "Couper", "Copier", "Coller", "Tout sélectionner",
    "Ouvrir", "Notifications",
]};

#[rustfmt::skip]
const DE: WordTable = Table { tag: "de", entries: [
    "Über {app}", "Einstellungen …", "Dienste", "{app} ausblenden", "Andere ausblenden", "Alle einblenden", "{app} beenden",
    "Fenster", "Im Dock ablegen", "Zoomen", "Alle nach vorne bringen",
    "Widerrufen", "Wiederholen", "Ausschneiden", "Kopieren", "Einsetzen", "Alles auswählen",
    "Öffnen", "Mitteilungen",
]};

#[rustfmt::skip]
const IT: WordTable = Table { tag: "it", entries: [
    "Informazioni su {app}", "Impostazioni…", "Servizi", "Nascondi {app}", "Nascondi altre", "Mostra tutte", "Esci da {app}",
    "Finestra", "Contrai", "Zoom", "Porta tutto in primo piano",
    "Annulla", "Ripeti", "Taglia", "Copia", "Incolla", "Seleziona tutto",
    "Apri", "Notifiche",
]};

#[rustfmt::skip]
const JA: WordTable = Table { tag: "ja", entries: [
    "{app}について", "設定…", "サービス", "{app}を非表示", "ほかを非表示", "すべてを表示", "{app}を終了",
    "ウインドウ", "しまう", "拡大/縮小", "すべてを手前に移動",
    "取り消す", "やり直す", "カット", "コピー", "ペースト", "すべてを選択",
    "開く", "通知",
]};

#[rustfmt::skip]
const ZH_HANS: WordTable = Table { tag: "zh-Hans", entries: [
    "关于 {app}", "设置…", "服务", "隐藏 {app}", "隐藏其他", "全部显示", "退出 {app}",
    "窗口", "最小化", "缩放", "前置全部窗口",
    "撤销", "重做", "剪切", "拷贝", "粘贴", "全选",
    "打开", "通知",
]};

#[rustfmt::skip]
const ZH_HANT: WordTable = Table { tag: "zh-Hant", entries: [
    "關於 {app}", "設定…", "服務", "隱藏 {app}", "隱藏其他", "顯示全部", "結束 {app}",
    "視窗", "縮到最小", "縮放", "全部移至最前",
    "還原", "重做", "剪下", "拷貝", "貼上", "全選",
    "打開", "通知",
]};

#[rustfmt::skip]
const KO: WordTable = Table { tag: "ko", entries: [
    "{app}에 관하여", "설정…", "서비스", "{app} 가리기", "기타 가리기", "모두 보기", "{app} 종료",
    "윈도우", "최소화", "확대/축소", "모두 앞으로 가져오기",
    "실행 취소", "실행 복귀", "오려두기", "복사하기", "붙여넣기", "전체 선택",
    "열기", "알림",
]};

#[rustfmt::skip]
const NL: WordTable = Table { tag: "nl", entries: [
    "Over {app}", "Instellingen…", "Voorzieningen", "Verberg {app}", "Verberg andere", "Toon alles", "Stop {app}",
    "Venster", "Minimaliseer", "Zoom", "Haal alles naar voren",
    "Herstel", "Opnieuw", "Knip", "Kopieer", "Plak", "Selecteer alles",
    "Open", "Meldingen",
]};

#[rustfmt::skip]
const RU: WordTable = Table { tag: "ru", entries: [
    "О программе {app}", "Настройки…", "Службы", "Скрыть {app}", "Скрыть остальные", "Показать все", "Завершить {app}",
    "Окно", "Свернуть", "Масштаб", "Все окна на передний план",
    "Отменить", "Повторить", "Вырезать", "Скопировать", "Вставить", "Выбрать все",
    "Открыть", "Уведомления",
]};

/// Turkish inflects what it attaches to, and a suffix cannot attach to
/// a placeholder: the name stays whole and the word that follows it
/// carries the case ("{app} Uygulamasını Gizle").
#[rustfmt::skip]
const TR: WordTable = Table { tag: "tr", entries: [
    "{app} Hakkında", "Ayarlar…", "Servisler", "{app} Uygulamasını Gizle", "Diğerlerini Gizle", "Tümünü Göster", "{app} Uygulamasından Çık",
    "Pencere", "Küçült", "Yakınlaştır", "Tümünü Öne Getir",
    "Geri Al", "Yinele", "Kes", "Kopyala", "Yapıştır", "Tümünü Seç",
    "Aç", "Bildirimler",
]};

#[rustfmt::skip]
const PL: WordTable = Table { tag: "pl", entries: [
    "{app} – informacje", "Ustawienia…", "Usługi", "Ukryj {app}", "Ukryj pozostałe", "Pokaż wszystkie", "Zakończ {app}",
    "Okno", "Minimalizuj", "Powiększ", "Wszystkie na wierzch",
    "Cofnij", "Przywróć", "Wytnij", "Kopiuj", "Wklej", "Zaznacz wszystko",
    "Otwórz", "Powiadomienia",
]};

#[rustfmt::skip]
const SV: WordTable = Table { tag: "sv", entries: [
    "Om {app}", "Inställningar…", "Tjänster", "Göm {app}", "Göm övriga", "Visa alla", "Avsluta {app}",
    "Fönster", "Minimera", "Zooma", "Lägg alla överst",
    "Ångra", "Gör om", "Klipp ut", "Kopiera", "Klistra in", "Markera allt",
    "Öppna", "Notiser",
]};

/// The sixteen tables, English first — the source every other falls to.
pub static TABLES: [WordTable; 16] =
    [EN, PT, PT_PT, ES, FR, DE, IT, JA, ZH_HANS, ZH_HANT, KO, NL, RU, TR, PL, SV];

/// The framework's words as a catalog — [`Words::for_locale`] picks
/// from it; a tool lists it.
pub static CATALOG: Catalog<Word, WORDS> = Catalog::new(&TABLES);

/// The app's say: a word for a locale, or `None` to leave the table.
type Hook = Rc<dyn Fn(Word, &Locale) -> Option<Cow<'static, str>>>;

thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
}

/// Installs the app's say over the framework's words: `hook` is asked
/// first for every word, in the locale in effect, and answers `Some`
/// with its own wording — a template, with `{app}` where the name
/// goes, for a word that names the app — or `None` to let the table
/// speak. One per thread, like the theme; a second install replaces
/// the first. Install it before the menu bar is set, or set the bar
/// again after.
pub fn install(hook: impl Fn(Word, &Locale) -> Option<Cow<'static, str>> + 'static) {
    HOOK.with(|slot| *slot.borrow_mut() = Some(Rc::new(hook)));
}

/// Removes the installed hook: the tables speak again.
pub fn uninstall() {
    HOOK.with(|slot| *slot.borrow_mut() = None);
}

/// The words resolved for ONE locale: a table picked once, and the hook
/// as it stood when they were resolved. Cheap to make where a menu is
/// built or a notification posted; not something a body needs per row.
pub struct Words {
    strings: Strings<Word, WORDS>,
    locale: Locale,
    hook: Option<Hook>,
}

impl Words {
    /// The words for `locale` — the shell passes the locale in effect.
    pub fn for_locale(locale: &Locale) -> Words {
        Words {
            strings: CATALOG.pick(locale),
            locale: locale.clone(),
            hook: HOOK.with(|slot| slot.borrow().clone()),
        }
    }

    /// The source words, for a place that must read the same on every
    /// machine (a probe, a test).
    pub fn english() -> Words {
        Self::for_locale(&Locale::default())
    }

    /// The locale these words were resolved for.
    pub fn locale(&self) -> &Locale {
        &self.locale
    }

    /// The tag of the table that answered.
    pub fn tag(&self) -> &'static str {
        self.strings.tag()
    }

    /// The word — the hook's if it spoke, else the table's, borrowed.
    pub fn get(&self, word: Word) -> Cow<'static, str> {
        if let Some(hook) = &self.hook
            && let Some(answer) = hook(word, &self.locale)
        {
            return answer;
        }
        Cow::Borrowed(self.strings.get(word))
    }

    /// The word with the app's name in it, where the language puts it.
    pub fn titled(&self, word: Word, app: &str) -> String {
        fill(&self.get(word), &[("app", app)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The enum's order is the tables' order, every table holds every
    /// word, and the placeholder stands exactly where a word names the
    /// app — in every language.
    #[test]
    fn every_word_indexes_its_own_row() {
        for (index, word) in Word::ALL.iter().enumerate() {
            assert_eq!(word.index(), index, "{word:?}");
        }
        assert_eq!(Word::ALL.len(), WORDS);
        for table in &TABLES {
            for (word, entry) in Word::ALL.iter().zip(table.entries.iter()) {
                assert!(!entry.is_empty(), "{}: {word:?} is written", table.tag);
                assert_eq!(
                    entry.contains("{app}"),
                    word.names_the_app(),
                    "{}: {word:?} names the app where the template says",
                    table.tag
                );
            }
        }
        let tags: Vec<_> = CATALOG.tags().collect();
        assert_eq!(tags[0], "en", "English is the source");
        assert_eq!(tags.len(), 16);
    }

    #[test]
    fn brazilian_and_european_portuguese_are_two_tables() {
        assert_eq!(Words::for_locale(&Locale::new("pt-BR")).get(Word::Quit), "Encerrar {app}");
        assert_eq!(Words::for_locale(&Locale::new("pt")).get(Word::Quit), "Encerrar {app}");
        assert_eq!(Words::for_locale(&Locale::new("pt-PT")).get(Word::Quit), "Sair de {app}");
        assert_eq!(Words::for_locale(&Locale::new("pt-BR")).tag(), "pt");
        assert_eq!(Words::for_locale(&Locale::new("pt-AO")).tag(), "pt", "another region falls to the language");
    }

    #[test]
    fn a_word_with_the_app_in_it_is_titled_in_the_languages_order() {
        assert_eq!(Words::english().titled(Word::Quit, "Bunny"), "Quit Bunny");
        assert_eq!(Words::for_locale(&Locale::new("de")).titled(Word::Quit, "Bunny"), "Bunny beenden");
        assert_eq!(Words::for_locale(&Locale::new("ja")).titled(Word::Hide, "Bunny"), "Bunnyを非表示");
        assert_eq!(Words::for_locale(&Locale::new("pt-BR")).titled(Word::About, "Bunny"), "Sobre o Bunny");
        assert_eq!(Words::for_locale(&Locale::new("en")).titled(Word::Copy, "Bunny"), "Copy", "no name, no change");
    }

    #[test]
    fn an_unknown_language_falls_to_english_and_chinese_regions_find_their_script() {
        assert_eq!(Words::for_locale(&Locale::new("eo")).tag(), "en");
        assert_eq!(Words::for_locale(&Locale::new("zh-TW")).get(Word::Window), "視窗");
        assert_eq!(Words::for_locale(&Locale::new("zh-CN")).get(Word::Window), "窗口");
        assert_eq!(Words::for_locale(&Locale::parse("gsw,fr")).get(Word::Window), "Fenêtre");
    }

    #[test]
    fn an_installed_hook_outranks_the_table_and_a_none_leaves_it() {
        install(|word, locale| {
            (word == Word::Quit && locale.language() == "pt").then(|| Cow::Borrowed("Sair do {app}"))
        });
        let words = Words::for_locale(&Locale::new("pt-BR"));
        assert_eq!(words.titled(Word::Quit, "Bunny"), "Sair do Bunny", "the app's wording");
        assert_eq!(words.get(Word::Copy), "Copiar", "a None leaves the table to speak");
        assert_eq!(Words::english().titled(Word::Quit, "Bunny"), "Quit Bunny", "another locale is not answered");
        uninstall();
        assert_eq!(Words::for_locale(&Locale::new("pt-BR")).titled(Word::Quit, "Bunny"), "Encerrar Bunny");
    }
}
