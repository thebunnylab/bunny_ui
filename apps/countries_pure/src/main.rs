//
//  CountriesApp.swift (demo headless) — countries-pure
//
//  `@main struct MainApp: App` + `WindowGroup { appEnvironment.rootView }` —
//  o papel do AppDelegate (bootstrap, lifecycle, deep links) é exercitado
//  aqui na mão, num roteiro de 4 atos: render → sceneDidBecomeActive →
//  deep link → destino.
//

use countries_core::Core::DeepLinksHandler::{DeepLink, DeepLinksHandler, RealDeepLinksHandler};
use countries_core::DependencyInjection::AppEnvironment::AppEnvironment;
use countries_core::Repositories::Database::CountriesDBRepository::CountryDBModel;
use countries_core::Repositories::Models::ApiModel;
use bunny_ui::prelude::*;

use countries_pure::root::root_view;
use countries_pure::ui::country_details::CountryDetails;

fn main() {
    // `AppEnvironment.bootstrap()` — mesma ordem de montagem do Swift
    let app = AppEnvironment::bootstrap();

    // `@Environment(\.locale)` a partir do root
    let mut environment = EnvironmentValues::default();
    environment.locale = Locale::new("en");
    let runtime = Runtime::with_environment(environment);
    let ctx = runtime.context();

    let root = root_view(&app, &ctx);

    // 1. Lançamento — `.on_appear` do default_view dispara o
    //    load_countries_list; a "rede" é o MockUrlSession servindo o MockedData
    println!("━━━ 1. launch (notRequested → onAppear carrega a lista do mock) ━━━");
    println!("{}", runtime.render_stable(&root));

    // 2. `sceneDidBecomeActive()` — system.isActive + resolveStatus(push)
    app.systemEventsHandler.sceneDidBecomeActive();
    println!("\n━━━ 2. sceneDidBecomeActive (lista carregada do mock) ━━━");
    println!("{}", runtime.render_stable(&root));

    // 3. Deep link — o equivalente do "push_with_deeplink.apns" do simulador:
    //    roteia countryCode + detailsSheet e o on_change empurra o país na
    //    NavigationStack
    let deep_links_handler = RealDeepLinksHandler::new(app.diContainer.clone());
    deep_links_handler.open(DeepLink::ShowCountryFlag {
        alpha3Code: "USA".into(),
    });
    println!("\n━━━ 3. deep link .showCountryFlag(alpha3Code: \"USA\") ━━━");
    println!("{}", runtime.render_stable(&root));

    // 4. O destino empurrado — o NavigationStack fake não monta destinos
    //    (são description-only), então montamos o CountryDetails do USA aqui;
    //    a sheet abre porque o deep link setou routing.countryDetails.detailsSheet
    let country = ApiModel::mockedCountries().remove(0).dbModel(); // USA
    let details = CountryDetails::new(country).inject(Rc::new(app.diContainer.clone()));
    println!("\n━━━ 4. destination: CountryDetails(USA) + sheet do ModalFlagView ━━━");
    println!("{}", runtime.render_stable(&details));
}

#[cfg(test)]
mod tests {
    //! Smoke test da demo — um teste só percorrendo os 4 atos, porque os
    //! slots de efeito (um por callsite, via `#[track_caller]`) vivem num
    //! mapa global por thread: rodar o fluxo duas vezes deixaria os slots
    //! quentes e as mudanças não disparariam de novo.

    use super::*;

    #[test]
    fn demo_flow_renders_the_expected_trees() {
        let app = AppEnvironment::bootstrap();
        let mut environment = EnvironmentValues::default();
        environment.locale = Locale::new("en");
        let runtime = Runtime::with_environment(environment);
        let ctx = runtime.context();
        let root = root_view(&app, &ctx);

        // Ato 1: launch — a lista do mock carrega via onAppear
        let act1 = runtime.render_stable(&root);
        assert!(
            act1.contains("NavigationStack (path: 0)"),
            "path vazio no lançamento"
        );
        assert!(act1.contains("List (3)"), "os 3 países do mock");
        assert!(act1.contains("United States"));
        assert!(
            act1.contains("[.blur(radius: 10)]"),
            "aparecência pré-ativação"
        );

        // Ato 2: cena ativa — o blur do RootViewAppearance cai pra zero
        app.systemEventsHandler.sceneDidBecomeActive();
        let act2 = runtime.render_stable(&root);
        assert!(
            act2.contains("[.blur(radius: 0)]"),
            "aparecência pós-ativação"
        );

        // Ato 3: deep link — o on_change empurra o USA na NavigationStack
        let deep_links_handler = RealDeepLinksHandler::new(app.diContainer.clone());
        deep_links_handler.open(DeepLink::ShowCountryFlag {
            alpha3Code: "USA".into(),
        });
        let act3 = runtime.render_stable(&root);
        assert!(act3.contains("NavigationStack (path: 1)"), "país empurrado");

        // Ato 4: o destino — CountryDetails com a sheet do ModalFlagView
        let country = ApiModel::mockedCountries().remove(0).dbModel();
        let details = CountryDetails::new(country).inject(Rc::new(app.diContainer.clone()));
        let act4 = runtime.render_stable(&details);
        assert!(act4.contains("CountryDetails"));
        assert!(act4.contains("Basic Info"));
        assert!(act4.contains("Sheet"));
        assert!(act4.contains("ModalFlagView"));
        // a bandeira CARREGA: o ImageView é recriado a cada render, mas o
        // estado dele ancora na identidade estrutural e revive — sem isso a
        // view ficava presa no Text("") do notRequested para sempre
        assert!(act4.contains("Image (UIImage"));
    }
}
