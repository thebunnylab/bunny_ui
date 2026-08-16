//
//  CountryCell.swift — CountriesSwiftUI
//
//  Desvios do port (documentados):
//  - `LocaleReader` não existe aqui: views leem o locale direto do `ctx` —
//    o container do Swift servia para reach de fora da view.
//  - `Inspection` (ViewInspector) fica de fora — a demo headless inspeciona.
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
    fn body(&self, ctx: &Context) -> impl View {
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
