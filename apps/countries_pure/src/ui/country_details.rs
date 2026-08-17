//
//  CountryDetails.swift — CountriesSwiftUI
//
//  `Routing` is a view-local struct, so here it is snake_case; the core's
//  `CountriesListRouting` (the value that lives in the AppState) stays
//  camelCase — it is the seam of the mirrored port.
//
//  The view carries the `country` (data, not state), so it is not `Copy`
//  like CountriesList — the closures clone `self` (cheap: the state fields
//  are handles). It is the case that asks for a copyable `Stored<T>` for
//  config in the future.
//
//  The `content` `match` is a `OneOf4`; the `loadedView` `if`s become
//  `Option`s in the tuple — they flatten into nothing.
//

use countries_core::Core::AppState::CountryDetailsRouting;
use countries_core::DependencyInjection::DIContainer::DIContainer;
use countries_core::Foundation::URL;
use countries_core::Repositories::Models::DBModel;
use bunny_ui::prelude::*;

use crate::ui::{
    detail_row::DetailRow, error_view::ErrorView, image_view::ImageView,
    modal_flag_view::ModalFlagView,
};

/// `CountryDetails.Routing { var detailsSheet: Bool = false }`
#[derive(Clone, Default, PartialEq)]
struct Routing {
    details_sheet: bool,
}

#[derive(Clone)]
pub struct CountryDetails {
    country: DBModel::Country,
    details: State<Loadable<DBModel::CountryDetails>>,
    routing_state: State<Routing>,
}

impl CountryDetails {
    /// `CountryDetails(country:)` — `details` default `.notRequested`
    pub fn new(country: DBModel::Country) -> Self {
        Self::with_details(country, Loadable::NotRequested)
    }

    /// `CountryDetails(country:details:)`
    pub fn with_details(
        country: DBModel::Country,
        details: Loadable<DBModel::CountryDetails>,
    ) -> Self {
        Self {
            country,
            details: State::new(details),
            routing_state: State::new(Routing::default()),
        }
    }
}

impl Component for CountryDetails {
    fn body(self, ctx: &Context) -> impl View {
        let injected = ctx.environment::<DIContainer>();
        let locale = ctx.environment::<Locale>();
        // State is Copy: it leaves the struct before `self` moves into the content
        let routing_state = self.routing_state;
        let title = self.country.name_locale(locale);

        self.content(ctx)
            .nav_bar_title(title)
            .on_receive(Self::routing_update(&injected), move |routing| {
                routing_state.set(Routing {
                    details_sheet: routing.detailsSheet,
                });
            })
    }
}

// MARK: - Content

impl CountryDetails {
    fn content(self, ctx: &Context) -> impl UnaryView {
        match self.details.get() {
            Loadable::NotRequested => OneOf4::A(self.default_view(ctx)),
            Loadable::IsLoading(..) => OneOf4::B(self.loading_view()),
            Loadable::Loaded(details) => OneOf4::C(self.loaded_view(ctx, details)),
            Loadable::Failed(error) => OneOf4::D(self.failed_view(ctx, error)),
        }
    }

    fn default_view(self, ctx: &Context) -> impl UnaryView {
        let injected = ctx.environment::<DIContainer>();
        text("").on_appear(move || self.load_country_details(&injected, false))
    }

    fn loading_view(self) -> impl UnaryView {
        vstack((
            progress_view().progress_style(ProgressViewStyle::Circular),
            button(text("Cancel loading"), move || {
                self.details.update(|details| details.cancelLoading())
            }),
        ))
    }

    fn failed_view(self, ctx: &Context, error: LoadError) -> impl UnaryView {
        let injected = ctx.environment::<DIContainer>();
        ErrorView::new(
            error,
            Rc::new(move || self.load_country_details(&injected, true)),
        )
    }

    fn loaded_view(self, ctx: &Context, country_details: DBModel::CountryDetails) -> impl UnaryView {
        let injected = ctx.environment::<DIContainer>();

        let currencies = (!country_details.currencies.is_empty())
            .then(|| Self::currencies_section_view(country_details.currencies.clone()));
        let neighbors = country_details
            .neighbors
            .clone()
            .filter(|neighbors| !neighbors.is_empty())
            .map(|neighbors| self.clone().neighbors_section_view(ctx, neighbors));
        // the view carries DATA (Clone, not Copy): each sub-view takes its
        // own copy — explicit, and cheap at the size this has
        let sheet_view = self.clone();

        list_content((
            self.country
                .flag
                .clone()
                .map(|url| self.clone().flag_view(ctx, url)),
            self.clone().basic_info_section_view(country_details.clone()),
            currencies,
            neighbors,
        ))
        .list_style(ListStyle::Grouped)
        .sheet(
            self.routing_binding(&injected)
                .member(|r| r.details_sheet, |r, value| r.details_sheet = value),
            move |sheet_ctx| erased(sheet_view.clone().modal_details_view(sheet_ctx)),
        )
    }
}

// MARK: - Displaying Content

impl CountryDetails {
    /// `flagView(url:)` — the headless runtime's `onTapGesture` fires at
    /// render (there is no finger), so the sheet opens just like the demo.
    fn flag_view(self, ctx: &Context, url: URL) -> impl UnaryView {
        let injected = ctx.environment::<DIContainer>();
        hstack((
            spacer(),
            ImageView::new(url)
                .frame(120.0, 80.0)
                .on_tap(move || self.show_country_details_sheet(&injected)),
            spacer(),
        ))
    }

    /// `basicInfoSectionView(countryDetails:)` — takes the data owned: in
    /// edition 2021 a returned `impl View` would capture the lifetime of the
    /// `&CountryDetails` and would not prove `'static`.
    fn basic_info_section_view(self, country_details: DBModel::CountryDetails) -> impl UnaryView {
        section(
            text("Basic Info"),
            (
                DetailRow::new(text(self.country.alpha3Code.clone()), text("Code")),
                DetailRow::new(
                    text(format!("Population {}", self.country.population)),
                    text("Population"),
                ),
                DetailRow::new(text(country_details.capital.clone()), text("Capital")),
            ),
        )
    }

    /// `currenciesSectionView(currencies:)`
    fn currencies_section_view(currencies: Vec<DBModel::Currency>) -> impl UnaryView {
        section(
            text("Currencies"),
            (for_each(
                currencies,
                |currency| currency.code.clone(),
                |currency| DetailRow::new(text(currency.title()), text(currency.code.clone())),
            ),),
        )
    }

    /// `neighborsSectionView(neighbors:)`
    fn neighbors_section_view(self, ctx: &Context, neighbors: Vec<DBModel::Country>) -> impl UnaryView {
        let locale = ctx.environment::<Locale>();
        section(
            text("Neighboring countries"),
            (for_each(
                neighbors,
                |country| country.alpha3Code.clone(),
                move |country| {
                    let label = DetailRow::new(
                        text(country.name_locale(locale.clone())),
                        text(String::new()),
                    );
                    navigation_link(Self::neighbour_details_view(country.clone()), label)
                },
            ),),
        )
    }

    /// `neighbourDetailsView(country:)` — the destination of the neighbor's link.
    fn neighbour_details_view(country: DBModel::Country) -> CountryDetails {
        CountryDetails::new(country)
    }

    /// `modalDetailsView()` — the sheet's content (an erased boundary).
    fn modal_details_view(self, ctx: &Context) -> impl UnaryView {
        let injected = ctx.environment::<DIContainer>();
        let details_sheet = self
            .routing_binding(&injected)
            .member(|r| r.details_sheet, |r, value| r.details_sheet = value);
        ModalFlagView::new(self.country.clone(), details_sheet).inject(Rc::new(injected))
    }
}

// MARK: - Side Effects

impl CountryDetails {
    /// `loadCountryDetails(forceReload:)`
    fn load_country_details(&self, injected: &DIContainer, force_reload: bool) {
        let interactor = injected.interactors.countries.clone();
        let country = self.country.clone();
        self.details
            .binding()
            .load(async move { interactor.loadCountryDetails(country, force_reload).await });
    }

    /// `showCountryDetailsSheet()`
    fn show_country_details_sheet(&self, injected: &DIContainer) {
        injected
            .appState
            .update(|state| state.routing.countryDetails.detailsSheet = true);
    }
}

// MARK: - Routing

impl CountryDetails {
    /// `$routingState.dispatched(to: injected.appState, \.routing.countryDetails)`
    fn routing_binding(&self, injected: &DIContainer) -> Binding<Routing> {
        Binding::dispatched(
            &injected.appState,
            |state| Routing {
                details_sheet: state.routing.countryDetails.detailsSheet,
            },
            |state, routing| state.routing.countryDetails.detailsSheet = routing.details_sheet,
        )
    }

    /// `routingUpdate` — `injected.appState.updates(for: \.routing.countryDetails)`
    fn routing_update(injected: &DIContainer) -> AnyPublisher<CountryDetailsRouting> {
        injected
            .appState
            .updates(|state| state.routing.countryDetails.clone())
    }
}
