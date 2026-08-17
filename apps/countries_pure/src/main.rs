//
//  CountriesApp.swift (headless demo) — countries-pure
//
//  `@main struct MainApp: App` + `WindowGroup { appEnvironment.rootView }` —
//  the AppDelegate's role (bootstrap, lifecycle, deep links) is exercised
//  here by hand, in a script of 4 acts: render → sceneDidBecomeActive →
//  deep link → destination.
//

use countries_core::Core::DeepLinksHandler::{DeepLink, DeepLinksHandler, RealDeepLinksHandler};
use countries_core::DependencyInjection::AppEnvironment::AppEnvironment;
use countries_core::Repositories::Database::CountriesDBRepository::CountryDBModel;
use countries_core::Repositories::Models::ApiModel;
use bunny_ui::prelude::*;

use countries_pure::root::root_view;
use countries_pure::ui::country_details::CountryDetails;

fn main() {
    // `AppEnvironment.bootstrap()` — same assembly order as the Swift
    let app = AppEnvironment::bootstrap();

    // `@Environment(\.locale)` starting from the root
    let mut environment = EnvironmentValues::default();
    environment.locale = Locale::new("en");
    let runtime = Runtime::with_environment(environment);
    let ctx = runtime.context();

    let root = root_view(&app, &ctx);

    // 1. Launch — the default_view's `.on_appear` fires
    //    load_countries_list; the "network" is the MockUrlSession serving the MockedData
    println!("━━━ 1. launch (notRequested → onAppear loads the list from the mock) ━━━");
    println!("{}", runtime.render_stable(&root));

    // 2. `sceneDidBecomeActive()` — system.isActive + resolveStatus(push)
    app.systemEventsHandler.sceneDidBecomeActive();
    println!("\n━━━ 2. sceneDidBecomeActive (list loaded from the mock) ━━━");
    println!("{}", runtime.render_stable(&root));

    // 3. Deep link — the equivalent of the simulator's "push_with_deeplink.apns":
    //    routes countryCode + detailsSheet and the on_change pushes the country
    //    onto the NavigationStack
    let deep_links_handler = RealDeepLinksHandler::new(app.diContainer.clone());
    deep_links_handler.open(DeepLink::ShowCountryFlag {
        alpha3Code: "USA".into(),
    });
    println!("\n━━━ 3. deep link .showCountryFlag(alpha3Code: \"USA\") ━━━");
    println!("{}", runtime.render_stable(&root));

    // 4. The pushed destination — the fake NavigationStack does not mount
    //    destinations (they are description-only), so we mount the USA CountryDetails here;
    //    the sheet opens because the deep link set routing.countryDetails.detailsSheet
    let country = ApiModel::mockedCountries().remove(0).dbModel(); // USA
    let details = CountryDetails::new(country).inject(Rc::new(app.diContainer.clone()));
    println!("\n━━━ 4. destination: CountryDetails(USA) + ModalFlagView sheet ━━━");
    println!("{}", runtime.render_stable(&details));
}

#[cfg(test)]
mod tests {
    //! Smoke test of the demo — a single test walking the 4 acts, because
    //! the effect slots (one per callsite, via `#[track_caller]`) live in a
    //! global per-thread map: running the flow twice would leave the slots
    //! warm and the changes would not fire again.

    use super::*;

    #[test]
    fn demo_flow_renders_the_expected_trees() {
        let app = AppEnvironment::bootstrap();
        let mut environment = EnvironmentValues::default();
        environment.locale = Locale::new("en");
        let runtime = Runtime::with_environment(environment);
        let ctx = runtime.context();
        let root = root_view(&app, &ctx);

        // Act 1: launch — the mock list loads via onAppear
        let act1 = runtime.render_stable(&root);
        assert!(
            act1.contains("NavigationStack (path: 0)"),
            "empty path at launch"
        );
        assert!(act1.contains("List (3)"), "the 3 mock countries");
        assert!(act1.contains("United States"));
        assert!(
            act1.contains("[.blur(radius: 10)]"),
            "pre-activation appearance"
        );

        // Act 2: scene active — the RootViewAppearance blur drops to zero
        app.systemEventsHandler.sceneDidBecomeActive();
        let act2 = runtime.render_stable(&root);
        assert!(
            act2.contains("[.blur(radius: 0)]"),
            "post-activation appearance"
        );

        // Act 3: deep link — the on_change pushes the USA onto the NavigationStack
        let deep_links_handler = RealDeepLinksHandler::new(app.diContainer.clone());
        deep_links_handler.open(DeepLink::ShowCountryFlag {
            alpha3Code: "USA".into(),
        });
        let act3 = runtime.render_stable(&root);
        assert!(act3.contains("NavigationStack (path: 1)"), "country pushed");

        // Act 4: the destination — CountryDetails with the ModalFlagView sheet
        let country = ApiModel::mockedCountries().remove(0).dbModel();
        let details = CountryDetails::new(country).inject(Rc::new(app.diContainer.clone()));
        let act4 = runtime.render_stable(&details);
        assert!(act4.contains("CountryDetails"));
        assert!(act4.contains("Basic Info"));
        assert!(act4.contains("Sheet"));
        assert!(act4.contains("ModalFlagView"));
        // the flag LOADS: the ImageView is recreated on every render, but its
        // state anchors on the structural identity and revives — without that
        // the view stayed stuck in the notRequested Text("") forever
        assert!(act4.contains("Image (UIImage"));
    }
}
