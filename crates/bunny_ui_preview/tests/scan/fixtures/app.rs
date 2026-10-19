// A whole app, without a marked preview. Strings and macro templates are not entries.
const EXAMPLE: &str = "bunny_ui::app!(fake)";
macro_rules! example { () => { bunny_ui::app!(fake); }; }
fn root() -> impl View { text("App") }
bunny_ui::app!(root, bunny_ui::AppConfig::new().size(420.0, 640.0));

#[preview]
fn app() -> impl View { root() }

bunny_ui::declare_preview!(app, bunny_preview__app, "App", "", "app");
