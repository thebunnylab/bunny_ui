//! O CountriesSwiftUI de verdade numa janela nativa — lista com texto da
//! plataforma e rolagem real:
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example countries_window
//! ```
//!
//! Limitações desta fase (anotadas, não escondidas): `navigation_link`
//! ainda não monta destino (clicar numa row não navega); `.searchable`/
//! `.refreshable`/`.toolbar` são inertes; imagens são caixas 40×40.

use std::rc::Rc;

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
use bunny_ui_macos::CoreTextEngine;
use countries_core::DependencyInjection::AppEnvironment::AppEnvironment;
use countries_pure::root_view;

fn main() {
    // mesma ordem de montagem da demo headless
    let app = AppEnvironment::bootstrap();
    let mut environment = EnvironmentValues::default();
    environment.locale = Locale::new("en");
    let runtime =
        Runtime::with_environment(environment).text_engine(Rc::new(CoreTextEngine::new()));
    let ctx = runtime.context();
    let root = root_view(&app, &ctx);

    // a cena ativa antes do primeiro frame (blur → 0, push resolve) — o
    // hook real de applicationDidBecomeActive é etapa de lifecycle futura
    app.systemEventsHandler.sceneDidBecomeActive();

    bunny_ui_macos::run_window_with(
        "Countries",
        Size { width: 480.0, height: 360.0 },
        runtime,
        root,
    );
}
