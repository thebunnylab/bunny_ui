//
//  ModalFlagView.swift — CountriesSwiftUI
//
//  Port deviations: `Inspection` (ViewInspector) stays out — the headless
//  demo already is the inspection.
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
    /// The modifiers live inside the `map`: applying them on an `Option`
    /// (zero-or-one arity) would decorate the nothing when `None` — the
    /// arity in the type forbids that, so the title and the toolbar follow
    /// the content that exists.
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

    /// `closeButton` — `Button("Close") { isDisplayed = false }` (the fake
    /// runtime's `.toolbar` is inert and never mounts it, as in the engine).
    fn close_button(self) -> impl UnaryView {
        button(text("Close"), move || self.is_displayed.set(false))
    }
}
