//! The ready-made update control, in each of its states.
//!
//! The example starts an updater that never reaches a feed, then puts
//! each state on the pill with a button: what a person sees while a real
//! update runs. In an app built by `bunny build` with an updater
//! configuration, `app!` starts the real one, and the app only places
//! `update_pill()`.
//!
//! cargo run -p bunny-ui --features updater --example updater

use std::sync::Arc;

use bunny_ui::prelude::*;
use bunny_ui::updater::release::{Format, InstallProgress, Installed, Offer, ProductId, ReleaseAsset, Transfer};
use bunny_ui::updater::ui::{UpdateBannerExt, update_pill};
use bunny_ui::updater::{Feed, Fetch, HttpReply, Schedule, UpdateError, UpdateState, UpdaterConfig};

/// A transport with no network: the states come from the buttons.
struct Nowhere;

impl Fetch for Nowhere {
    fn get(&self, _url: &str) -> Result<HttpReply, UpdateError> {
        Err(UpdateError::Transport("this example asks no feed".into()))
    }
}

fn offer() -> Offer {
    Offer {
        version: "1.1.0".parse().expect("a version"),
        asset: ReleaseAsset { url: "https://example.invalid/app.dmg".into(), signature: String::new(), format: Format::Dmg, size: None, sha256: None },
        notes: None,
    }
}

fn show(state: UpdateState) -> impl Fn() + Clone {
    move || {
        if let Some(shown) = bunny_ui::updater::state() {
            shown.set(state.clone());
        }
    }
}

fn home() -> impl View {
    let downloading = |share: u64| UpdateState::Installing(InstallProgress::Downloading(Transfer::new(share, Some(100))));
    vstack!(
        hstack!(text("My app").font(Font::Title), spacer(), update_pill()),
        text("Put the pill in a state:").foreground_color(bunny_ui::theme::current().fg_secondary),
        button(text("Idle"), show(UpdateState::Idle)),
        button(text("Available"), show(UpdateState::Available(offer()))),
        button(text("Downloading 40%"), show(downloading(40))),
        button(text("Verifying"), show(UpdateState::Installing(InstallProgress::Verifying))),
        button(text("Ready to restart"), show(UpdateState::ReadyToRestart {
            version: "1.1.0".parse().expect("a version"),
            outcome: Box::new(Installed::ReadyOnRestart),
        })),
        button(text("Failed"), show(UpdateState::Failed)),
        button(text("Retired"), show(UpdateState::Retired {
            minimum: "1.0.1".parse().expect("a version"),
            upgrade: Some(Box::new(offer())),
        })),
        spacer(),
    )
    .alignment(HorizontalAlignment::Leading)
    .padding()
    .update_banner()
}

bunny_ui::app!(home, bunny_ui::AppConfig::new().title("updater").size(420.0, 420.0));

fn main() {
    let config = UpdaterConfig::new(
        Feed::Server("https://example.invalid".into()),
        ProductId::parse("bunny-updater-example").expect("a product"),
        "1.0.0".parse().expect("a version"),
    );
    bunny_ui::updater::start_with(config, Arc::new(Nowhere), Schedule::Manual);
    run()
}
