//
//  CountryCell.swift — CountriesSwiftUI
//
//  Port deviations (documented):
//  - `LocaleReader` does not exist here: views read the locale straight from
//    the `ctx` — the Swift container was there for reach from outside the view.
//  - `Inspection` (ViewInspector) stays out — the headless demo inspects.
//

use countries_core::Repositories::Models::DBModel;
use bunny_ui::prelude::*;

#[derive(Clone)]
pub struct CountryCell {
    country: DBModel::Country,
}

impl CountryCell {
    /// `CountryCell(country:)`
    pub fn new(country: DBModel::Country) -> Self {
        Self { country }
    }
}

impl Component for CountryCell {
    fn body(self, ctx: &Context) -> impl View {
        let locale = ctx.environment::<Locale>();
        vstack((
            text(self.country.name_locale(locale)).font(Font::Title),
            text(format!("Population {}", self.country.population)).font(Font::Caption),
        ))
        .alignment(HorizontalAlignment::Leading)
        .padding()
        .frame_max(f64::INFINITY, 60.0, Alignment::Leading)
    }
}
