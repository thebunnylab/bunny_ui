//
//  CountriesApp.swift (rootView) — countries-pure
//
//  `extension AppEnvironment { var rootView: some View }` becomes a free fn:
//  there is no macro here to mirror the syntax, and the orphan rule does not
//  even need dodging. The `@ViewBuilder`'s `if app.isRunningTests` is the
//  `Either` (_ConditionalContent); the `if isStub` becomes an `Option` in the tuple.
//
//  Swift's `attachEnvironmentOverrides(onChange:)` handler exists to reset
//  the ViewRouting when locale/sizeCategory change — in the headless port
//  the modifier is inert and nobody fires the handler, so it does not
//  appear.
//

use countries_core::DependencyInjection::AppEnvironment::AppEnvironment;
use bunny_ui::prelude::*;

use crate::ui::countries_list::CountriesList;
use crate::ui::root_view_modifier::RootViewAppearance;

/// `appEnvironment.rootView`
pub fn root_view(app: &AppEnvironment, _ctx: &Context) -> impl View {
    if app.isRunningTests {
        Either::First(vstack((text("Running unit tests"),)))
    } else {
        Either::Second(vstack((
            CountriesList::new()
                .modifier(RootViewAppearance::new(&app.diContainer))
                .model_container(Rc::new(app.modelContainer.clone()))
                .attach_environment_overrides_on_change()
                .inject(Rc::new(app.diContainer.clone())),
            app.modelContainer
                .isStub()
                .then(|| text("⚠️ There is an issue with local database").font(Font::Caption2)),
        )))
    }
}
