//! The preview vocabulary — a view with the traits it is shown under.
//!
//! A `#[preview]` function answers one of these: the view it wants seen,
//! and under what — the colour scheme (or both, side by side), the size
//! or the device frame, the locale and the reading direction, the size
//! class, the device scale, the safe area, a theme of its own. The data
//! is what a gallery lists, what the Variants window crosses and what a
//! host applies to the scene it renders the view in — the same way
//! SwiftUI's `#Preview` traits ride beside the view, and like them it is
//! inert on its own: nothing here draws.
//!
//! ## What this ships
//!
//! - [`Preview`]: the view and its [`Traits`], with a builder per trait.
//! - [`Traits`]: the traits, resolved by [`Traits::variants`] into the
//!   scenes a `Both` scheme asks for, by [`Traits::proposal`] into the
//!   proposal a scene lays out under, and by [`Traits::apply`] onto a
//!   [`Runtime`] — its own theme, locale, direction, size class, scale
//!   and safe area, through the doors the shells already use.
//! - [`Device`]: the frames a preview is shown in, with each device's
//!   scale, safe area and size class.
//!
//! ## Wiring
//!
//! Nothing here is read by the runtime on its own: a host calls
//! [`Traits::apply`] on the scene it opens for a preview and lays the
//! view out under [`Traits::proposal`]. [`Runtime::theme`] /
//! [`Runtime::set_theme`] are the runtime's side of the scheme trait —
//! a scene renders under a theme of its own, quietly, so a light scene
//! and a dark one alternate on one thread without rebuilding each other.

use crate::layout::{Edges, Proposal, Px, Size};
use crate::runtime::Runtime;
use crate::theme::Theme;
use crate::view::View;
use motor::state::{LayoutDirection, Locale, SizeClass};

mod execution;
pub use execution::{ExecutionContext, ExecutionMode, ExecutionScope, ExecutionSubscription};

/// A view and the traits it is shown under.
#[derive(Clone, Debug)]
pub struct Preview<V: View> {
    /// The view.
    pub view: V,
    /// The traits.
    pub traits: Traits,
}

impl<V: View> Preview<V> {
    /// `view` under the default traits: light, sized to fit, the
    /// runtime's locale.
    pub fn new(view: V) -> Self {
        Self { view, traits: Traits::default() }
    }

    /// The view and the traits, apart.
    pub fn into_parts(self) -> (V, Traits) {
        (self.view, self.traits)
    }

    /// Shown under the light theme.
    pub fn light(mut self) -> Self {
        self.traits.scheme = Scheme::Light;
        self
    }

    /// Shown under the dark theme.
    pub fn dark(mut self) -> Self {
        self.traits.scheme = Scheme::Dark;
        self
    }

    /// Shown under both themes, as two scenes side by side.
    pub fn schemes(mut self) -> Self {
        self.traits.scheme = Scheme::Both;
        self
    }

    /// Shown under `theme` whatever the scheme says — a brand's own
    /// tokens, a high-contrast variant.
    pub fn theme(mut self, theme: Theme) -> Self {
        self.traits.theme = Some(theme);
        self
    }

    /// Laid out in exactly `width` × `height` logical pixels.
    pub fn size(mut self, width: Px, height: Px) -> Self {
        self.traits.size = Sizing::Fixed(Size { width, height });
        self
    }

    /// Laid out at the size the view asks for — a component, not a screen.
    pub fn size_that_fits(mut self) -> Self {
        self.traits.size = Sizing::Fits;
        self
    }

    /// Laid out in a device's frame, with its scale, safe area and size
    /// class unless a trait below says otherwise.
    pub fn device(mut self, device: Device) -> Self {
        self.traits.size = Sizing::Device(device);
        self
    }

    /// Shown under the locale `tag` names (`"pt-BR"`, `"ar"`): the words
    /// the view reads through the environment, and — unless [`Self::rtl`]
    /// or the locale itself says — the reading direction.
    pub fn locale(mut self, tag: &str) -> Self {
        self.traits.locale = Some(Locale::new(tag));
        self
    }

    /// Read right to left, whatever the locale says.
    pub fn rtl(mut self) -> Self {
        self.traits.direction = Some(LayoutDirection::RightToLeft);
        self
    }

    /// Shown in the compact size class — a phone's width.
    pub fn compact(mut self) -> Self {
        self.traits.size_class = Some(SizeClass::Compact);
        self
    }

    /// Rendered at `scale` device pixels per logical pixel.
    pub fn scale(mut self, scale: Px) -> Self {
        self.traits.scale = Some(scale);
        self
    }

    /// Shown behind `insets` of safe area — a notch, a home indicator.
    pub fn safe_area(mut self, insets: Edges) -> Self {
        self.traits.safe_area = Some(insets);
        self
    }
}

/// What a `#[preview]` function may answer: a bare view, shown under the
/// default traits, or a [`Preview`] carrying its own. The attribute's
/// entry calls this, so a function never names the wrapper unless it
/// has a trait to set.
pub trait IntoPreview {
    /// The view the preview shows.
    type View: View;

    /// The view with its traits.
    fn into_preview(self) -> Preview<Self::View>;
}

impl<V: View> IntoPreview for V {
    type View = V;

    fn into_preview(self) -> Preview<V> {
        Preview::new(self)
    }
}

impl<V: View> IntoPreview for Preview<V> {
    type View = V;

    fn into_preview(self) -> Preview<V> {
        self
    }
}

/// The traits a preview is shown under. Every trait but the scheme and
/// the sizing is optional: `None` leaves the runtime's own answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Traits {
    /// Light, dark, or both as two scenes.
    pub scheme: Scheme,
    /// A theme that wins over the scheme's.
    pub theme: Option<Theme>,
    /// How the view is sized.
    pub size: Sizing,
    /// The locale pinned on the scene.
    pub locale: Option<Locale>,
    /// The reading direction pinned on the scene (the locale's otherwise).
    pub direction: Option<LayoutDirection>,
    /// The horizontal size class (the device's, or regular, otherwise).
    pub size_class: Option<SizeClass>,
    /// Device pixels per logical pixel (the device's, or 1, otherwise).
    pub scale: Option<Px>,
    /// The safe area (the device's, or none, otherwise).
    pub safe_area: Option<Edges>,
}

impl Default for Traits {
    fn default() -> Self {
        Self {
            scheme: Scheme::Light,
            theme: None,
            size: Sizing::Fits,
            locale: None,
            direction: None,
            size_class: None,
            scale: None,
            safe_area: None,
        }
    }
}

impl Traits {
    /// The scenes these traits ask for: one, or two when the scheme is
    /// [`Scheme::Both`] — the light one first, every other trait shared.
    /// An explicit [`Traits::theme`] wins in both, as it does in one.
    pub fn variants(&self) -> Vec<Traits> {
        match self.scheme {
            Scheme::Both => vec![
                Traits { scheme: Scheme::Light, ..self.clone() },
                Traits { scheme: Scheme::Dark, ..self.clone() },
            ],
            Scheme::Light | Scheme::Dark => vec![self.clone()],
        }
    }

    /// The theme the scene renders under: the explicit one, else the
    /// scheme's (`Both` resolves to light — split it with
    /// [`Traits::variants`] first).
    pub fn resolved_theme(&self) -> Theme {
        self.theme.unwrap_or_else(|| match self.scheme {
            Scheme::Dark => Theme::dark(),
            Scheme::Light | Scheme::Both => Theme::light(),
        })
    }

    /// The device the sizing names, if it names one.
    pub fn device(&self) -> Option<Device> {
        match self.size {
            Sizing::Device(device) => Some(device),
            Sizing::Fixed(_) | Sizing::Fits => None,
        }
    }

    /// The frame the view is laid out in, when the traits fix one.
    pub fn frame(&self) -> Option<Size> {
        match self.size {
            Sizing::Fixed(size) => Some(size),
            Sizing::Device(device) => Some(device.frame()),
            Sizing::Fits => None,
        }
    }

    /// The proposal a scene lays the view out under: exact for a size or
    /// a device, unspecified when the view sizes itself.
    pub fn proposal(&self) -> Proposal {
        self.frame().map_or_else(Proposal::unspecified, Proposal::exact)
    }

    /// The size class in effect: the pinned one, else the device's, else
    /// regular.
    pub fn resolved_size_class(&self) -> SizeClass {
        self.size_class
            .or_else(|| self.device().map(Device::size_class))
            .unwrap_or_default()
    }

    /// The device scale in effect: the pinned one, else the device's, else 1.
    pub fn resolved_scale(&self) -> Px {
        self.scale.or_else(|| self.device().map(Device::scale)).unwrap_or(1.0)
    }

    /// The safe area in effect: the pinned one, else the device's, else none.
    pub fn resolved_safe_area(&self) -> Edges {
        self.safe_area
            .or_else(|| self.device().map(Device::safe_area))
            .unwrap_or(Edges::ZERO)
    }

    /// Puts the traits on `runtime` through the doors the shells use:
    /// its own theme, the pinned locale and direction, the size class,
    /// the device scale and the safe area. Idempotent — each door answers
    /// whether it moved, and nothing moves twice.
    pub fn apply(&self, runtime: &Runtime) {
        runtime.set_theme(Some(self.resolved_theme()));
        runtime.set_locale(self.locale.clone());
        runtime.set_layout_direction(self.direction);
        runtime.set_size_class(self.resolved_size_class());
        runtime.set_device_scale(self.resolved_scale());
        runtime.set_safe_area(self.resolved_safe_area());
    }
}

/// The colour scheme a preview is shown under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Scheme {
    /// The light theme.
    #[default]
    Light,
    /// The dark theme.
    Dark,
    /// Both, as two scenes side by side.
    Both,
}

/// How a preview is sized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sizing {
    /// Exactly this many logical pixels.
    Fixed(Size),
    /// What the view asks for — a component.
    Fits,
    /// A device's frame — a screen.
    Device(Device),
}

/// The device frames a preview is shown in — logical points, the
/// portrait orientation, with each device's scale, safe area and size
/// class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Device {
    /// iPhone 16: 393 × 852 at 3×.
    IPhone16,
    /// iPhone 16 Pro Max: 440 × 956 at 3×.
    IPhone16ProMax,
    /// iPad Pro 11″: 834 × 1210 at 2×.
    IPadPro11,
    /// Pixel 9: 412 × 923 dp at 2.625×.
    Pixel9,
    /// A desktop window: 1024 × 768 at 2×, no safe area.
    MacWindow,
}

impl Device {
    /// Every device, in the order a picker offers them.
    pub const ALL: [Device; 5] = [
        Device::IPhone16,
        Device::IPhone16ProMax,
        Device::IPadPro11,
        Device::Pixel9,
        Device::MacWindow,
    ];

    /// The device's name, as a picker shows it.
    pub const fn name(self) -> &'static str {
        match self {
            Device::IPhone16 => "iPhone 16",
            Device::IPhone16ProMax => "iPhone 16 Pro Max",
            Device::IPadPro11 => "iPad Pro 11″",
            Device::Pixel9 => "Pixel 9",
            Device::MacWindow => "Mac window",
        }
    }

    /// The frame, in logical pixels, portrait.
    pub const fn frame(self) -> Size {
        match self {
            Device::IPhone16 => Size { width: 393.0, height: 852.0 },
            Device::IPhone16ProMax => Size { width: 440.0, height: 956.0 },
            Device::IPadPro11 => Size { width: 834.0, height: 1210.0 },
            Device::Pixel9 => Size { width: 412.0, height: 923.0 },
            Device::MacWindow => Size { width: 1024.0, height: 768.0 },
        }
    }

    /// Device pixels per logical pixel.
    pub const fn scale(self) -> Px {
        match self {
            Device::IPhone16 | Device::IPhone16ProMax => 3.0,
            Device::IPadPro11 | Device::MacWindow => 2.0,
            Device::Pixel9 => 2.625,
        }
    }

    /// What the screen is covered by at each edge: the status bar or the
    /// notch above, the home indicator or the gesture bar below.
    pub const fn safe_area(self) -> Edges {
        match self {
            Device::IPhone16 | Device::IPhone16ProMax => {
                Edges { top: 59.0, trailing: 0.0, bottom: 34.0, leading: 0.0 }
            }
            Device::IPadPro11 => Edges { top: 24.0, trailing: 0.0, bottom: 20.0, leading: 0.0 },
            Device::Pixel9 => Edges { top: 24.0, trailing: 0.0, bottom: 24.0, leading: 0.0 },
            Device::MacWindow => Edges::ZERO,
        }
    }

    /// The horizontal size class the device's width earns, portrait.
    pub const fn size_class(self) -> SizeClass {
        match self {
            Device::IPhone16 | Device::IPhone16ProMax | Device::Pixel9 => SizeClass::Compact,
            Device::IPadPro11 | Device::MacWindow => SizeClass::Regular,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::text;

    #[test]
    fn the_builders_write_the_traits_they_name() {
        let preview = Preview::new(text("x"))
            .dark()
            .device(Device::IPhone16)
            .locale("ar")
            .rtl()
            .compact()
            .scale(2.0)
            .theme(Theme::light());
        let traits = preview.traits.clone();
        assert_eq!(traits.scheme, Scheme::Dark);
        assert_eq!(traits.size, Sizing::Device(Device::IPhone16));
        assert_eq!(traits.locale, Some(Locale::new("ar")));
        assert_eq!(traits.direction, Some(LayoutDirection::RightToLeft));
        assert_eq!(traits.size_class, Some(SizeClass::Compact));
        assert_eq!(traits.scale, Some(2.0));
        assert_eq!(traits.resolved_theme(), Theme::light(), "an explicit theme wins over the scheme");
        assert_eq!(traits.frame(), Some(Device::IPhone16.frame()));
        assert_eq!(traits.proposal(), Proposal::exact(Device::IPhone16.frame()));
        assert_eq!(traits.resolved_safe_area(), Device::IPhone16.safe_area());
        let (_, traits) = preview.size_that_fits().into_parts();
        assert_eq!(traits.proposal(), Proposal::unspecified());
        assert_eq!(traits.resolved_scale(), 2.0, "a pinned scale outlives the device");
        assert_eq!(traits.resolved_safe_area(), Edges::ZERO);
    }

    #[test]
    fn both_schemes_are_two_scenes_light_first_and_the_defaults_are_the_runtimes() {
        let both = Preview::new(text("x")).schemes().size(200.0, 100.0).traits;
        let variants = both.variants();
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].scheme, Scheme::Light);
        assert_eq!(variants[1].scheme, Scheme::Dark);
        assert_eq!(variants[1].resolved_theme(), Theme::dark());
        assert!(variants.iter().all(|traits| traits.size == both.size));
        let one = Traits::default();
        assert_eq!(one.variants().len(), 1);
        assert_eq!(one.resolved_theme(), Theme::light());
        assert_eq!(one.resolved_size_class(), SizeClass::Regular);
        assert_eq!(one.resolved_scale(), 1.0);
        assert_eq!(one.proposal(), Proposal::unspecified());
    }

    #[test]
    fn a_bare_view_and_a_preview_both_answer_into_preview() {
        let bare = text("x").into_preview();
        assert_eq!(bare.traits, Traits::default());
        let traced = Preview::new(text("x")).dark().into_preview();
        assert_eq!(traced.traits.scheme, Scheme::Dark);
    }

    #[test]
    fn every_device_has_a_portrait_frame_and_its_class() {
        for device in Device::ALL {
            let frame = device.frame();
            assert!(frame.width < frame.height || device == Device::MacWindow, "{}", device.name());
            assert!(device.scale() >= 2.0);
            assert_eq!(
                device.size_class() == SizeClass::Compact,
                frame.width < 600.0,
                "{}",
                device.name()
            );
        }
    }
}
