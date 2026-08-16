//
//  CountryDetails.swift — CountriesSwiftUI
//
//  O `Routing` é uma struct local da view, então aqui ele é snake_case;
//  o `CountriesListRouting` do core (o valor que vive no AppState) continua
//  camelCase — é o seam do port espelhado.
//
//  A view carrega o `country` (dado, não estado), então não é `Copy` como a
//  CountriesList — os closures clonam `self` (barato: os campos de estado
//  são handles). É o caso que pede um `Stored<T>` copiável para config no
//  futuro.
//
//  O `match` do `content` é um `OneOf4`; os `if`s do `loadedView` viram
//  `Option` na tupla — achatam em nada.
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
        // State é Copy: sai da struct antes de `self` entrar no content
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
        // a view carrega DADOS (Clone, não Copy): cada sub-view leva a sua
        // cópia — explícito, e barato no tamanho que isso tem
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
    /// `flagView(url:)` — o `onTapGesture` do runtime headless dispara no
    /// render (não há dedo), então a sheet abre igual à demo.
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

    /// `basicInfoSectionView(countryDetails:)` — recebe os dados owned: em
    /// edition 2021 um `impl View` de retorno capturaria o lifetime do
    /// `&CountryDetails` e não provaria `'static`.
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

    /// `neighbourDetailsView(country:)` — o destino do link do vizinho.
    fn neighbour_details_view(country: DBModel::Country) -> CountryDetails {
        CountryDetails::new(country)
    }

    /// `modalDetailsView()` — o conteúdo da sheet (borda apagada).
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
