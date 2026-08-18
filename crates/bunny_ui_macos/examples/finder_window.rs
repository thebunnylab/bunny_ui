//! A complete "go to file" — the first real SCREEN of bunny_ui.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example finder_window
//! ```
//!
//! Everything together in one place: translucent backdrop, floating
//! panel, REAL search field (focus, caret, IME, clipboard), reactive
//! filtered list (type and it responds — only this body re-runs), rows
//! with hover and click without button chrome, scrolling with clip and
//! thumb. The footer shows what the click "opened".

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

const PANEL_W: f64 = 640.0;
const PANEL_H: f64 = 480.0;
const PANEL_TOP: f64 = 120.0;

// the finder actions — key becomes intent in main()'s keymap
const SELECT_NEXT: ActionId = ActionId("finder.select_next");
const SELECT_PREV: ActionId = ActionId("finder.select_prev");
const PAGE_FORWARD: ActionId = ActionId("finder.page_forward");
const PAGE_BACK: ActionId = ActionId("finder.page_back");
const OPEN: ActionId = ActionId("finder.open");
const OPEN_SPLIT: ActionId = ActionId("finder.open_split");
const DISMISS: ActionId = ActionId("finder.dismiss");

const CLEAR: Color = Color::rgba(0, 0, 0, 0);

/// (name, directory, recent?) — the mock of a plausible project.
const FILES: &[(&str, &str, bool)] = &[
    ("main.rs", "src/", true),
    ("layout.rs", "crates/ui/src/", true),
    ("raster.rs", "crates/ui/src/", false),
    ("runtime.rs", "crates/ui/src/", true),
    ("reconciler.rs", "crates/ui/src/", false),
    ("text_engine.rs", "crates/ui/src/", false),
    ("text_input.rs", "crates/ui/src/", false),
    ("views.rs", "crates/ui/src/", false),
    ("modifier.rs", "crates/ui/src/", false),
    ("identity.rs", "crates/engine/src/", false),
    ("state.rs", "crates/engine/src/", true),
    ("combine.rs", "crates/engine/src/", false),
    ("loadable.rs", "crates/engine/src/", false),
    ("ffi.rs", "crates/shell/src/", false),
    ("window.rs", "crates/shell/src/", false),
    ("text.rs", "crates/shell/src/", false),
    ("Cargo.toml", "", false),
    ("README.md", "", false),
    ("countries_list.rs", "apps/countries/src/ui/", false),
    ("country_cell.rs", "apps/countries/src/ui/", false),
    ("country_details.rs", "apps/countries/src/ui/", false),
    ("image_view.rs", "apps/countries/src/ui/", false),
    ("error_view.rs", "apps/countries/src/ui/", false),
    ("detail_row.rs", "apps/countries/src/ui/", false),
    ("root.rs", "apps/countries/src/", false),
    ("finder_window.rs", "examples/", false),
    ("counter_window.rs", "examples/", false),
    ("benchmark.rs", "tools/src/", false),
    ("glyphs.rs", "tools/src/", false),
    ("palette.rs", "tools/src/", false),
    (
        "deep_file.rs",
        "a/very/long/nested/path/that/keeps/going/and/going/until/it/cannot/possibly/fit/",
        false,
    ),
];

/// Case-insensitive subsequence with the matched positions (coalesced
/// byte ranges) — the honest floor of a fuzzy, with real highlighting.
fn match_ranges(haystack: &str, needle: &str) -> Option<Vec<(usize, usize)>> {
    if needle.is_empty() {
        return Some(Vec::new());
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut haystack = haystack.char_indices();
    'wanted: for wanted in needle.chars() {
        let wanted = wanted.to_ascii_lowercase();
        for (index, candidate) in haystack.by_ref() {
            if candidate.to_ascii_lowercase() == wanted {
                let end = index + candidate.len_utf8();
                match ranges.last_mut() {
                    Some((_, last_end)) if *last_end == index => *last_end = end,
                    _ => ranges.push((index, end)),
                }
                continue 'wanted;
            }
        }
        return None;
    }
    Some(ranges)
}

#[derive(Clone)]
struct Row {
    name: String,
    dir: String,
    recent: bool,
    name_ranges: Vec<(usize, usize)>,
    dir_ranges: Vec<(usize, usize)>,
}

fn divider() -> impl UnaryView {
    spacer().frame(PANEL_W, 1.0).background_color(theme::divider())
}

/// The details popover — a card past the panel's edge: on the mac it
/// rides its own child panel and leaves the window when the screen has
/// the room (escape or a click outside closes it).
fn details_card(name: String, full: String) -> Erased {
    erased(
        vstack!(
            text(name).font(Font::Headline),
            text(full)
                .font(Font::Subheadline)
                .monospaced()
                .foreground_color(theme::fg_secondary()),
            text("escape or click outside to close")
                .font(Font::Footnote)
                .foreground_color(theme::fg_faint()),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(6.0)
        .padding_length(12.0)
        .background_color(theme::panel())
        .corner_radius(10.0)
        .border(theme::border(), 1.0)
        .shadow(24.0),
    )
}

#[derive(Clone, Copy)]
struct Finder {
    query: State<String>,
    opened: State<String>,
    dark: State<bool>,
    /// Selected index in the FILTERED list (clamped at display time).
    selected: State<usize>,
    /// A click on the SELECTED row opens its details popover — which
    /// steps OUTSIDE the window when the screen has the room.
    details: State<bool>,
}

impl Component for Finder {
    fn body(self, _ctx: &Context) -> impl View {
        let query = self.query.get();
        let items: Vec<Row> = FILES
            .iter()
            .filter_map(|(name, dir, recent)| {
                let full = format!("{dir}{name}");
                let ranges = match_ranges(&full, &query)?;
                // splits the full path's ranges between dir and name
                let split = dir.len();
                let mut dir_ranges = Vec::new();
                let mut name_ranges = Vec::new();
                for (start, end) in ranges {
                    if end <= split {
                        dir_ranges.push((start, end));
                    } else if start >= split {
                        name_ranges.push((start - split, end - split));
                    } else {
                        dir_ranges.push((start, split));
                        name_ranges.push((0, end - split));
                    }
                }
                Some(Row {
                    name: name.to_string(),
                    dir: dir.to_string(),
                    recent: *recent,
                    name_ranges,
                    dir_ranges,
                })
            })
            .collect();
        let count = items.len();
        let opened = self.opened;
        let selected = self.selected;
        let details = self.details;
        // the read registers the dependency: moving selection repaints the row
        let selected_index = selected.get().min(count.saturating_sub(1));

        let header = hstack!(
            icon(symbol::CHEVRON_RIGHT).font(Font::Headline).foreground_color(theme::accent()),
            text_field("Search files by name…", self.query.binding()).monospaced().auto_focus(),
        )
        .spacing(10.0)
        .alignment(VerticalAlignment::Center)
        .padding_length(10.0);

        let labels: Vec<String> =
            items.iter().map(|row| format!("{}{}", row.dir, row.name)).collect();
        let indexed: Vec<(usize, Row)> = items.into_iter().enumerate().collect();
        let results = list(
            indexed,
            |(_, row)| format!("{}{}", row.dir, row.name),
            move |(index, row)| {
                let row = row.clone();
                let label = format!("{}{}", row.dir, row.name);
                let file_name = row.name.clone();
                let is_selected = *index == selected_index;
                let base = hstack!(
                    // the platform's file icon, crisp at row size — the
                    // workspace picks the representation for 16pt
                    image(file_icon(label.as_str())).resizable().frame(16.0, 16.0),
                    // name: MIDDLE ellipsis, 240 cap as the anatomy dictates
                    text(row.name)
                        .foreground_color(theme::fg())
                        .highlight(row.name_ranges, theme::accent())
                        .truncation_mode(Truncation::Middle)
                        .frame_max(240.0, f64::INFINITY, Alignment::Leading),
                    // path: fills the rest, ellipsis at the START (the
                    // end of the path is what matters)
                    text(row.dir)
                        .font(Font::Subheadline)
                        .monospaced()
                        .foreground_color(theme::fg_secondary())
                        .highlight(row.dir_ranges, theme::accent())
                        .truncation_mode(Truncation::Start)
                        .frame_max(f64::INFINITY, f64::INFINITY, Alignment::Leading),
                    row.recent.then(|| {
                        text("recent").font(Font::Footnote).foreground_color(theme::fg_faint())
                    }),
                )
                .spacing(8.0)
                .alignment(VerticalAlignment::Center)
                .padding_edge(Edge::Leading, 12.0)
                .padding_edge(Edge::Trailing, 12.0)
                .padding_edge(Edge::Top, 7.0)
                .padding_edge(Edge::Bottom, 7.0)
                // keyboard selection paints the base background; hover
                // refines on top (the same contract as Styled)
                .background_color(if is_selected { theme::row_pressed() } else { CLEAR })
                .background_hovered(theme::row_hover())
                .background_pressed(theme::row_pressed())
                // the row breathes: hover and selection fade, and the
                // row slides when the filter reorders the list
                .animated(Spring::snappy())
                // a click on the selected row shows its details; on any
                // other it opens, as always
                .on_click({
                    let label = label.clone();
                    move || {
                        if is_selected {
                            details.set(true);
                        } else {
                            opened.set(label.clone());
                        }
                    }
                });
                if is_selected {
                    let full = label.clone();
                    erased(base.popover(details.binding(), Side::Trailing, move |_| {
                        details_card(file_name.clone(), full.clone())
                    }))
                } else {
                    erased(base)
                }
            },
        )
        // arrow keys move the selection below the fold — the region
        // follows just enough to keep it visible, FLYING there through
        // the spring; the wheel stays free and cancels the flight
        .animated(Spring::smooth())
        .scroll_target(labels.get(selected_index).cloned().unwrap_or_default());

        let body = if count == 0 {
            Either::Second(
                text("No matches")
                    .font(Font::Callout)
                    .foreground_color(theme::fg_faint())
                    .frame_max(f64::INFINITY, f64::INFINITY, Alignment::Center),
            )
        } else {
            Either::First(results)
        };

        let opened_name = self.opened.get();
        let status = if opened_name.is_empty() {
            "click a row to open".to_string()
        } else {
            format!("opened: {opened_name}")
        };
        let dark_on = self.dark.get();
        let footer = hstack!(
            text!("{count} files")
                .font(Font::Subheadline)
                .monospaced()
                .foreground_color(theme::fg_secondary()),
            // LIVE retheme: the install rebuilds the scene on the next pass
            button(
                text(if dark_on { "light" } else { "dark" }).font(Font::Footnote),
                move || {
                    let next = !self.dark.get();
                    self.dark.set(next);
                    theme::install(if next { Theme::dark() } else { Theme::light() });
                },
            ),
            spacer(),
            text(status).font(Font::Subheadline).monospaced().foreground_color(theme::fg_secondary()),
        )
        .spacing(12.0)
        .alignment(VerticalAlignment::Center)
        .padding_edge(Edge::Leading, 12.0)
        .padding_edge(Edge::Trailing, 12.0)
        .padding_edge(Edge::Top, 8.0)
        .padding_edge(Edge::Bottom, 8.0);

        // opens the selected one; the handlers capture the current body's
        // FILTERED list — new filter = body re-runs = fresh captures
        let open_at = move |prefix: &'static str| {
            if let Some(label) = labels.get(selected.get().min(count.saturating_sub(1))) {
                opened.set(format!("{prefix}{label}"));
            }
        };
        let query_state = self.query;
        let panel = vstack!(header, divider(), body, divider(), footer)
            .alignment(HorizontalAlignment::Leading)
            .frame(PANEL_W, PANEL_H)
            .background_color(theme::panel())
            .corner_radius(9.0)
            .border(theme::border(), 1.0)
            // the floating panel finally casts its shadow
            .shadow(24.0)
            // the finder keys answer only while the panel is mounted
            .key_context("finder")
            // ↓/↑ with wrap — they work WHILE typing (the gate consumes)
            .on_action(SELECT_NEXT, move || {
                if count > 0 {
                    details.set(false);
                    selected.set((selected.get().min(count - 1) + 1) % count)
                }
            })
            .on_action(SELECT_PREV, move || {
                if count > 0 {
                    details.set(false);
                    selected.set((selected.get().min(count - 1) + count - 1) % count)
                }
            })
            .on_action(PAGE_FORWARD, move || {
                if count > 0 {
                    details.set(false);
                    selected.set((selected.get() + 10).min(count - 1))
                }
            })
            .on_action(PAGE_BACK, move || {
                details.set(false);
                selected.set(selected.get().saturating_sub(10))
            })
            .on_action(OPEN, {
                let open_at = open_at.clone();
                move || open_at("")
            })
            .on_action(OPEN_SPLIT, move || open_at("split: "))
            .on_action(DISMISS, move || {
                query_state.set(String::new());
                selected.set(0);
            })
            // new filter = selection returns to the top
            .on_change(move || query_state.get(), false, move |_, _| selected.set(0));

        zstack!(
            spacer().background_color(theme::backdrop()),
            vstack!(spacer().frame(1.0, PANEL_TOP), panel, spacer()),
        )
    }
}

fn main() {
    let runtime = Runtime::new()
        .text_engine(Rc::new(bunny_ui_macos::CoreTextEngine::new()))
        .image_engine(Rc::new(bunny_ui_macos::CoreGraphicsImageEngine::new()));
    // the app keymap: key → intent (the handlers live in the screen)
    runtime.bind_in("finder", KeyPattern::key(Key::Down), SELECT_NEXT);
    runtime.bind_in("finder", KeyPattern::key(Key::Up), SELECT_PREV);
    runtime.bind_in("finder", KeyPattern::key(Key::PageDown), PAGE_FORWARD);
    runtime.bind_in("finder", KeyPattern::key(Key::PageUp), PAGE_BACK);
    runtime.bind_in("finder", KeyPattern::key(Key::Enter), OPEN);
    runtime.bind_in("finder", KeyPattern::command(Key::Enter), OPEN_SPLIT);
    runtime.bind_in("finder", KeyPattern::key(Key::Escape), DISMISS);

    bunny_ui_macos::run_window_with(
        "Finder",
        Size { width: 760.0, height: 640.0 },
        runtime,
        Finder {
            query: State::new(String::new()),
            opened: State::new(String::new()),
            dark: State::new(false),
            selected: State::new(0),
            details: State::new(false),
        },
    );
}
