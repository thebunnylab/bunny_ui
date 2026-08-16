//
//  CountriesApp.swift (rootView) — countries-pure
//
//  `extension AppEnvironment { var rootView: some View }` vira uma free fn:
//  aqui não há macro para espelhar a sintaxe, e a orphan rule nem precisa
//  ser driblada. O `if app.isRunningTests` do `@ViewBuilder` é o
//  `Either` (_ConditionalContent); o `if isStub` vira `Option` na tupla.
//
//  O handler de `attachEnvironmentOverrides(onChange:)` do Swift existe
//  para resetar o ViewRouting quando locale/sizeCategory mudam — no port
//  headless o modifier é inert e ninguém dispara o handler, então ele não
//  aparece.
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
