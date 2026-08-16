//
//  CountriesList.swift — CountriesSwiftUI
//
//  Desvios do port (documentados):
//  - `LocaleReader` não existe aqui: views leem o locale direto do `ctx` —
//    o container do Swift servia para reach de fora da view.
//  - `Inspection` (ViewInspector) fica de fora — a demo headless inspeciona.
//  - Os `site`s de `on_change`/`on_receive` saem de `#[track_caller]` —
//    cada callsite é seu próprio slot, nada a nomear.
//
//  A view é toda `State<_>`, então deriva `Copy` e o body recebe `self`
//  POR VALOR: cada closure captura os campos que usa (captura disjunta) —
//  o espelho do que o Swift faz com structs, sem cerimônia nenhuma.
//
//  O `match` do `content` é um `OneOf4` — os quatro braços do `Loadable`
//  com nome próprio, no lugar do `_ConditionalContent` binário aninhado.
//

use countries_core::Core::AppState::{CountriesListRouting, PermissionStatus};
use countries_core::DependencyInjection::DIContainer::DIContainer;
use countries_core::Interactors::UserPermissionsInteractor::Permission;
use countries_core::Repositories::Models::DBModel;
use countries_core::Utilities::Helpers::StringLocalizedStandardContains;
use bunny_ui::prelude::*;

use crate::ui::{country_cell::CountryCell, error_view::ErrorView};

/// `CountriesList.Routing { var countryCode: String? }`
#[derive(Clone, Default, PartialEq)]
struct Routing {
    country_code: Option<String>,
}

#[derive(Clone, Copy)]
pub struct CountriesList {
    countries: State<Vec<DBModel::Country>>,
    countries_state: State<Loadable<()>>,
    can_request_push_permission: State<bool>,
    search_text: State<String>,
    navigation_path: State<NavigationPath>,
    routing_state: State<Routing>,
}

impl CountriesList {
    /// `CountriesList()` — `state` default `.notRequested`
    pub fn new() -> Self {
        Self::with_state(Loadable::NotRequested)
    }

    /// `CountriesList(state:)`
    pub fn with_state(state: Loadable<()>) -> Self {
        Self {
            countries: State::new(Vec::new()),
            countries_state: State::new(state),
            can_request_push_permission: State::new(false),
            search_text: State::new(String::new()),
            navigation_path: State::new(NavigationPath::new()),
            routing_state: State::new(Routing::default()),
        }
    }
}

impl Default for CountriesList {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for CountriesList {
    fn body(self, ctx: &Context) -> impl View {
        let injected = ctx.environment::<DIContainer>();

        navigation_stack(
            self.navigation_path.binding(),
            (self
                .content(ctx)
                .query(
                    self.search_text.get(),
                    self.countries.binding(),
                    build_query,
                )
                .navigation_title("Countries"),),
        )
        .on_receive(Self::routing_update(&injected), move |routing| {
            self.routing_state.set(Routing {
                country_code: routing.countryCode,
            });
        })
        .on_receive(
            Self::can_request_push_permission_update(&injected),
            move |can_request| self.can_request_push_permission.set(can_request),
        )
        .flips_for_right_to_left_layout_direction(true)
    }
}

// MARK: - Loading Content

impl CountriesList {
    /// `@ViewBuilder private var content`
    fn content(self, ctx: &Context) -> impl UnaryView {
        match self.countries_state.get() {
            Loadable::NotRequested => OneOf4::A(self.default_view(ctx)),
            Loadable::IsLoading(..) => OneOf4::B(Self::loading_view()),
            Loadable::Loaded(()) => OneOf4::C(self.loaded_view(ctx)),
            Loadable::Failed(error) => OneOf4::D(self.failed_view(ctx, error)),
        }
    }

    /// `defaultView()`
    fn default_view(self, ctx: &Context) -> impl UnaryView {
        let injected = ctx.environment::<DIContainer>();
        text("").on_appear(move || {
            if !self.countries.get().is_empty() {
                self.countries_state.set(Loadable::Loaded(()));
            }
            self.load_countries_list(&injected, false);
        })
    }

    /// `loadingView()`
    fn loading_view() -> impl UnaryView {
        progress_view().progress_style(ProgressViewStyle::Circular)
    }

    /// `failedView(_:)`
    fn failed_view(self, ctx: &Context, error: LoadError) -> impl UnaryView {
        let injected = ctx.environment::<DIContainer>();
        ErrorView::new(
            error,
            Rc::new(move || self.load_countries_list(&injected, true)),
        )
    }
}

// MARK: - Displaying Content

impl CountriesList {
    /// `loadedView()` — o `@ViewBuilder` com `if` + `List` vira uma tupla
    /// com `Option` (o `if let` do codegen) e o `TupleView` explícito que
    /// imprime o próprio nó.
    fn loaded_view(self, ctx: &Context) -> impl UnaryView {
        let injected = ctx.environment::<DIContainer>();

        let no_matches = (self.countries.get().is_empty() && !self.search_text.get().is_empty())
            .then(|| text("No matches found").font(Font::Footnote));

        let refresh_injected = injected.clone();
        let clear_injected = injected.clone();

        tuple((
            no_matches,
            list(
                self.countries.get(),
                |country| country.alpha3Code.clone(),
                |country| nav_link_value(country.clone(), CountryCell::new(country.clone())),
            )
            .navigation_destination()
            .searchable(self.search_text.binding())
            .refreshable(move || self.load_countries_list(&refresh_injected, true))
            .toolbar(toolbar_item(self.permissions_button(ctx)))
            .on_change(
                move || self.routing_state.get().country_code.clone(),
                true,
                move |_, code| {
                    let Some(code) = code else { return };
                    let Some(country) = self
                        .countries
                        .get()
                        .into_iter()
                        .find(|country| country.alpha3Code == *code)
                    else {
                        return;
                    };
                    self.navigation_path.update(|path| path.append(country));
                },
            )
            .on_change(
                move || self.navigation_path.get(),
                false,
                move |_, path| {
                    if !path.is_empty() {
                        self.routing_binding(&clear_injected)
                            .update(|r| r.country_code = None);
                    }
                },
            ),
        ))
    }

    /// `permissionsButton` — `@ViewBuilder if canRequestPushPermission { … }`
    fn permissions_button(self, ctx: &Context) -> Option<impl UnaryView> {
        let injected = ctx.environment::<DIContainer>();
        self.can_request_push_permission.get().then(|| {
            button(text("Allow Push"), move || {
                self.request_push_permission(&injected)
            })
        })
    }
}

// MARK: - Side Effects

impl CountriesList {
    /// `loadCountriesList(forceReload:)`
    fn load_countries_list(&self, injected: &DIContainer, force_reload: bool) {
        if !force_reload && !self.countries.get().is_empty() {
            return; // guard forceReload || countries.isEmpty else { return }
        }
        let interactor = injected.interactors.countries.clone();
        self.countries_state
            .binding()
            .load(interactor.refreshCountriesList());
    }

    /// `requestPushPermission()`
    fn request_push_permission(&self, injected: &DIContainer) {
        injected
            .interactors
            .userPermissions
            .request(Permission::PushNotifications);
    }
}

// MARK: - Routing

impl CountriesList {
    /// `$routingState.dispatched(to: injected.appState, \.routing.countriesList)`
    fn routing_binding(&self, injected: &DIContainer) -> Binding<Routing> {
        Binding::dispatched(
            &injected.appState,
            |state| Routing {
                country_code: state.routing.countriesList.countryCode.clone(),
            },
            |state, routing| {
                state.routing.countriesList = CountriesListRouting {
                    countryCode: routing.country_code,
                }
            },
        )
    }
}

// MARK: - State Updates

impl CountriesList {
    /// `routingUpdate` — `injected.appState.updates(for: \.routing.countriesList)`
    fn routing_update(injected: &DIContainer) -> AnyPublisher<CountriesListRouting> {
        injected
            .appState
            .updates(|state| state.routing.countriesList.clone())
    }

    /// `canRequestPushPermissionUpdate`
    fn can_request_push_permission_update(injected: &DIContainer) -> AnyPublisher<bool> {
        injected
            .appState
            .updates(|state| state.permissions.push)
            .map(|status| {
                matches!(
                    status,
                    PermissionStatus::NotRequested | PermissionStatus::Denied
                )
            })
    }
}

// MARK: - Query

/// `Query(filter: #Predicate { … }, sort: \.name)` — o builder do
/// `.query(searchText:results:)`.
fn build_query(search: String) -> Query<DBModel::Country> {
    Query::new(
        Rc::new(move |country: &DBModel::Country| {
            search.is_empty() || country.name.localizedStandardContains(&search)
        }),
        Rc::new(|country: &DBModel::Country| country.name.clone()),
    )
}
