//
//  ModalFlagView.swift — CountriesSwiftUI
//
//  Desvios do port: o `Inspection` (ViewInspector) fica de fora — a demo
//  headless já é a inspeção.
//

use countries_core::Foundation::URL;
use countries_core::Repositories::Models::DBModel;
use bunny_ui::prelude::*;

use crate::ui::image_view::ImageView;

#[derive(Clone)]
pub struct ModalFlagView {
    country: DBModel::Country,
    is_displayed: Binding<bool>,
}

impl ModalFlagView {
    /// `ModalFlagView(country:isDisplayed:)`
    pub fn new(country: DBModel::Country, is_displayed: Binding<bool>) -> Self {
        Self {
            country,
            is_displayed,
        }
    }
}

impl Component for ModalFlagView {
    /// Os modifiers moram dentro do `map`: aplicar num `Option` (aridade
    /// zero-ou-um) decoraria o nada quando `None` — a aridade no tipo barra
    /// isso, então o título e a toolbar seguem o conteúdo que existe.
    fn body(self, _ctx: &Context) -> impl View {
        let flag_item = self.country.flag.clone().map(|url| {
            self.clone()
                .flag_view(url)
                .navigation_title(self.country.name.clone())
                .toolbar(toolbar_item(self.clone().close_button()))
        });
        navigation_stack_content((flag_item,))
            .navigation_view_style()
            .attach_environment_overrides()
    }
}

// MARK: - Private views

impl ModalFlagView {
    /// `country.flag.map { url in HStack { … } }`
    fn flag_view(self, url: URL) -> impl UnaryView {
        hstack((spacer(), ImageView::new(url).frame(300.0, 200.0), spacer()))
    }

    /// `closeButton` — `Button("Close") { isDisplayed = false }` (o
    /// `.toolbar` do runtime fake é inert e nunca o monta, como no motor).
    fn close_button(self) -> impl UnaryView {
        button(text("Close"), move || self.is_displayed.set(false))
    }
}
