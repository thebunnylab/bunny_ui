//
//  ImageView.swift — CountriesSwiftUI
//
//  The `Loadable` switch becomes a `OneOf4` in the body — each arm with a
//  name of its own, in place of the binary `_ConditionalContent` that
//  SwiftUI's codegen nests. `Inspection` (ViewInspector) stays out: the
//  headless demo already is the inspection.
//

use countries_core::DependencyInjection::DIContainer::DIContainer;
use countries_core::Foundation::{UIImage, URL};
use bunny_ui::prelude::*;

#[derive(Clone)]
pub struct ImageView {
    image_url: URL,
    image: State<Loadable<UIImage>>,
}

impl ImageView {
    /// `ImageView(imageURL:)` — `image` default `.notRequested`
    pub fn new(image_url: URL) -> Self {
        Self::with_image(image_url, Loadable::NotRequested)
    }

    /// `ImageView(imageURL:image:)`
    pub fn with_image(image_url: URL, image: Loadable<UIImage>) -> Self {
        Self {
            image_url,
            image: State::new(image),
        }
    }
}

impl Component for ImageView {
    fn body(self, ctx: &Context) -> impl View {
        match self.image.get() {
            Loadable::NotRequested => OneOf4::A(self.default_view(ctx)),
            Loadable::IsLoading(..) => OneOf4::B(Self::loading_view()),
            Loadable::Loaded(image) => OneOf4::C(Self::loaded_view(image)),
            Loadable::Failed(error) => OneOf4::D(Self::failed_view(error)),
        }
    }
}

// MARK: - Side Effects

impl ImageView {
    /// `loadImage()` — `injected.interactors.images.load(image:url:)`
    fn load_image(&self, injected: &DIContainer) {
        injected
            .interactors
            .images
            .load(self.image.binding(), Some(self.image_url.clone()));
    }
}

// MARK: - Content

impl ImageView {
    fn default_view(self, ctx: &Context) -> impl UnaryView {
        let injected = ctx.environment::<DIContainer>();
        text("").on_appear(move || self.load_image(&injected))
    }

    fn loading_view() -> impl UnaryView {
        progress_view().progress_style(ProgressViewStyle::Circular)
    }

    fn failed_view(error: LoadError) -> impl UnaryView {
        let _ = error; // `error.localizedDescription` does not show in the Swift UI
        text("Unable to load image")
            .font(Font::Footnote)
            .multiline_text_alignment(TextAlignment::Center)
            .padding()
    }

    fn loaded_view(image: UIImage) -> impl UnaryView {
        image_ui(image).resizable().aspect_ratio(ContentMode::Fit)
    }
}
