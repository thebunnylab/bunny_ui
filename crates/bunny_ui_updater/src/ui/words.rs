//! The words of the update control, in the sixteen languages of the
//! framework's own words. The app picks none of them: the locale of the
//! view does.

use bunny_ui_core::catalog::{Catalog, Key, Table};

/// The words of the update control, in the order of every table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Word {
    /// `{version}`: the offer.
    UpdateTo,
    /// A stage that has no words of its own.
    Updating,
    /// `{percent}`: the whole percent of the download.
    DownloadingPercent,
    /// `{size}`: the size so far, when no total is known.
    DownloadingSize,
    /// The hash and the signature are checked.
    Verifying,
    /// The build is put in place.
    Installing,
    /// Windows: the helper waits for the app to quit.
    QuitToFinish,
    /// The new build waits for a restart.
    RestartToUpdate,
    /// The last attempt failed; a click shows the log.
    Failed,
    /// `{minimum}`: retired, with a way out.
    UpdateToKeepWorking,
    /// `{minimum}`: retired, with no way out.
    NoLongerSupported,
    /// The failure's action: the log in the file browser.
    ShowLog,
}

impl Word {
    pub const ALL: [Word; 12] = [Word::UpdateTo, Word::Updating, Word::DownloadingPercent, Word::DownloadingSize, Word::Verifying, Word::Installing, Word::QuitToFinish, Word::RestartToUpdate, Word::Failed, Word::UpdateToKeepWorking, Word::NoLongerSupported, Word::ShowLog];
}

impl Key for Word {
    const COUNT: usize = 12;
    fn index(self) -> usize {
        self as usize
    }
}

type WordTable = Table<12>;

const EN: WordTable = Table { tag: "en", entries: [
    "Update to {version}",
    "Updating…",
    "Downloading {percent}%",
    "Downloading {size}",
    "Verifying…",
    "Installing…",
    "Quit to finish updating",
    "Restart to update",
    "Update failed",
    "Update to {minimum} or later to keep working",
    "This build is no longer supported ({minimum} or later)",
    "Show Log",
] };

const PT: WordTable = Table { tag: "pt", entries: [
    "Atualizar para {version}",
    "Atualizando…",
    "Baixando {percent}%",
    "Baixando {size}",
    "Verificando…",
    "Instalando…",
    "Encerrar para concluir a atualização",
    "Reiniciar para atualizar",
    "A atualização falhou",
    "Atualizar para {minimum} ou posterior para continuar trabalhando",
    "Esta versão não é mais suportada ({minimum} ou posterior)",
    "Mostrar log",
] };

const PT_PT: WordTable = Table { tag: "pt-PT", entries: [
    "Atualizar para {version}",
    "A atualizar…",
    "A transferir {percent}%",
    "A transferir {size}",
    "A verificar…",
    "A instalar…",
    "Sair para concluir a atualização",
    "Reiniciar para atualizar",
    "A atualização falhou",
    "Atualizar para {minimum} ou posterior para continuar a trabalhar",
    "Este build já não é suportado ({minimum} ou posterior)",
    "Mostrar registo",
] };

const ES: WordTable = Table { tag: "es", entries: [
    "Actualizar a {version}",
    "Actualizando…",
    "Descargando {percent} %",
    "Descargando {size}",
    "Verificando…",
    "Instalando…",
    "Salir para terminar de actualizar",
    "Reiniciar para actualizar",
    "La actualización falló",
    "Actualizar a {minimum} o posterior para seguir trabajando",
    "Este build ya no es compatible ({minimum} o posterior)",
    "Mostrar registro",
] };

const FR: WordTable = Table { tag: "fr", entries: [
    "Mettre à jour vers {version}",
    "Mise à jour…",
    "Téléchargement {percent} %",
    "Téléchargement {size}",
    "Vérification…",
    "Installation…",
    "Quittez pour terminer la mise à jour",
    "Redémarrez pour mettre à jour",
    "Échec de la mise à jour",
    "Passez à {minimum} ou ultérieur pour continuer à travailler",
    "Ce build n'est plus pris en charge ({minimum} ou ultérieur)",
    "Afficher le journal",
] };

const DE: WordTable = Table { tag: "de", entries: [
    "Auf {version} aktualisieren",
    "Wird aktualisiert…",
    "Herunterladen {percent} %",
    "Herunterladen {size}",
    "Wird überprüft…",
    "Wird installiert…",
    "Beenden, um das Update abzuschließen",
    "Zum Aktualisieren neu starten",
    "Update fehlgeschlagen",
    "Aktualisieren Sie auf {minimum} oder neuer, um weiterzuarbeiten",
    "Dieser Build wird nicht mehr unterstützt ({minimum} oder neuer)",
    "Protokoll anzeigen",
] };

const IT: WordTable = Table { tag: "it", entries: [
    "Aggiorna a {version}",
    "Aggiornamento…",
    "Download {percent}%",
    "Download {size}",
    "Verifica…",
    "Installazione…",
    "Esci per completare l'aggiornamento",
    "Riavvia per aggiornare",
    "Aggiornamento non riuscito",
    "Aggiorna a {minimum} o successiva per continuare a lavorare",
    "Questa build non è più supportata ({minimum} o successiva)",
    "Mostra log",
] };

const JA: WordTable = Table { tag: "ja", entries: [
    "{version} にアップデート",
    "アップデート中…",
    "ダウンロード中 {percent}%",
    "ダウンロード中 {size}",
    "検証中…",
    "インストール中…",
    "終了してアップデートを完了",
    "再起動してアップデート",
    "アップデートに失敗しました",
    "引き続き使うには、{minimum} 以降にアップデートしてください",
    "このビルドはサポートされなくなりました（{minimum} 以降）",
    "ログを表示",
] };

const ZH_HANS: WordTable = Table { tag: "zh-Hans", entries: [
    "更新到 {version}",
    "正在更新…",
    "正在下载 {percent}%",
    "正在下载 {size}",
    "正在验证…",
    "正在安装…",
    "退出以完成更新",
    "重启以更新",
    "更新失败",
    "更新到 {minimum} 或更高版本以继续使用",
    "此版本已不再受支持（{minimum} 或更高版本）",
    "显示日志",
] };

const ZH_HANT: WordTable = Table { tag: "zh-Hant", entries: [
    "更新至 {version}",
    "更新中…",
    "正在下載 {percent}%",
    "正在下載 {size}",
    "正在驗證…",
    "正在安裝…",
    "結束以完成更新",
    "重新啟動以更新",
    "更新失敗",
    "更新至 {minimum} 或更新版本以繼續使用",
    "此版本已不再受支援（{minimum} 或更新版本）",
    "顯示記錄檔",
] };

const KO: WordTable = Table { tag: "ko", entries: [
    "{version}(으)로 업데이트",
    "업데이트하는 중…",
    "다운로드 중 {percent}%",
    "다운로드 중 {size}",
    "확인하는 중…",
    "설치하는 중…",
    "종료하여 업데이트 완료",
    "다시 시작하여 업데이트",
    "업데이트 실패",
    "계속 사용하려면 {minimum} 이상으로 업데이트하세요",
    "이 빌드는 더 이상 지원되지 않습니다 ({minimum} 이상)",
    "로그 보기",
] };

const NL: WordTable = Table { tag: "nl", entries: [
    "Bijwerken naar {version}",
    "Bijwerken…",
    "Downloaden {percent}%",
    "Downloaden {size}",
    "Verifiëren…",
    "Installeren…",
    "Afsluiten om de update af te ronden",
    "Herstarten om bij te werken",
    "Update mislukt",
    "Werk bij naar {minimum} of nieuwer om door te kunnen werken",
    "Deze build wordt niet meer ondersteund ({minimum} of nieuwer)",
    "Logboek tonen",
] };

const RU: WordTable = Table { tag: "ru", entries: [
    "Обновить до {version}",
    "Обновление…",
    "Загрузка {percent}%",
    "Загрузка {size}",
    "Проверка…",
    "Установка…",
    "Закройте приложение, чтобы завершить обновление",
    "Перезапустите для обновления",
    "Не удалось обновить",
    "Обновитесь до {minimum} или новее, чтобы продолжить работу",
    "Эта сборка больше не поддерживается ({minimum} или новее)",
    "Показать журнал",
] };

const TR: WordTable = Table { tag: "tr", entries: [
    "{version} sürümüne güncelle",
    "Güncelleniyor…",
    "İndiriliyor %{percent}",
    "İndiriliyor {size}",
    "Doğrulanıyor…",
    "Yükleniyor…",
    "Güncellemeyi bitirmek için çıkın",
    "Güncellemek için yeniden başlatın",
    "Güncelleme başarısız oldu",
    "Çalışmaya devam etmek için {minimum} veya daha yeni bir sürüme güncelleyin",
    "Bu build artık desteklenmiyor ({minimum} veya üstü)",
    "Günlüğü göster",
] };

const PL: WordTable = Table { tag: "pl", entries: [
    "Zaktualizuj do {version}",
    "Aktualizowanie…",
    "Pobieranie {percent}%",
    "Pobieranie {size}",
    "Weryfikowanie…",
    "Instalowanie…",
    "Zamknij aplikację, aby dokończyć aktualizację",
    "Uruchom ponownie, aby zaktualizować",
    "Aktualizacja nie powiodła się",
    "Zaktualizuj do wersji {minimum} lub nowszej, aby dalej pracować",
    "Ten build nie jest już obsługiwany ({minimum} lub nowszy)",
    "Pokaż dziennik",
] };

const SV: WordTable = Table { tag: "sv", entries: [
    "Uppdatera till {version}",
    "Uppdaterar…",
    "Hämtar {percent} %",
    "Hämtar {size}",
    "Verifierar…",
    "Installerar…",
    "Avsluta för att slutföra uppdateringen",
    "Starta om för att uppdatera",
    "Uppdateringen misslyckades",
    "Uppdatera till {minimum} eller senare för att kunna fortsätta arbeta",
    "Den här builden stöds inte längre ({minimum} eller senare)",
    "Visa logg",
] };

/// The tables, English first: the source and the fallback.
pub static WORDS: Catalog<Word, 12> =
    Catalog::new(&[EN, PT, PT_PT, ES, FR, DE, IT, JA, ZH_HANS, ZH_HANT, KO, NL, RU, TR, PL, SV]);
