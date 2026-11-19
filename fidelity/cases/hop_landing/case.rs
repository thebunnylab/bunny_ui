//! Written by `bunny import html` from bunny/index.html.

use crate::prelude::*;

fn picture_hop_icon_ink_1() -> ImageSource {
    ImageSource::from_bytes(include_bytes!("../../../../../../../hop/website/bunny/../assets/hop-icon-ink.svg").as_slice())
}

fn picture_hop_icon_light_2() -> ImageSource {
    ImageSource::from_bytes(include_bytes!("../../../../../../../hop/website/bunny/../assets/hop-icon-light.svg").as_slice())
}

fn picture_hop_logo_light_0() -> ImageSource {
    ImageSource::from_bytes(include_bytes!("../../../../../../../hop/website/bunny/../assets/hop-logo-light.svg").as_slice())
}

#[must_use]
pub fn view() -> impl View {
    vstack!(
        vstack!(
            hstack!(
                hstack!(
                    image(picture_hop_logo_light_0())
                    .resizable()
                    .frame_height_aligned(34.0, Alignment::TopLeading)
                    .clipped()
                    .test_id("img-9")
                    .on_click(|| {}),
                )
                .alignment(VerticalAlignment::Center)
                .test_id("a-8")
                .on_click(|| {}),
                spacer(),
                hstack!(
                    text("Try it")
                    .font_size(15.0)
                    .font_weight(Weight::Medium)
                    .foreground_color(Color::rgb(201, 201, 207))
                    .line_height(23.25)
                    .padding_edge(Edge::Top, 10.0)
                    .padding_edge(Edge::Trailing, 14.0)
                    .padding_edge(Edge::Bottom, 10.0)
                    .padding_edge(Edge::Leading, 14.0)
                    .corner_radius(10.0)
                    .test_id("a-11")
                    .on_click(|| {}),
                    text("Features")
                    .font_size(15.0)
                    .font_weight(Weight::Medium)
                    .foreground_color(Color::rgb(201, 201, 207))
                    .line_height(23.25)
                    .padding_edge(Edge::Top, 10.0)
                    .padding_edge(Edge::Trailing, 14.0)
                    .padding_edge(Edge::Bottom, 10.0)
                    .padding_edge(Edge::Leading, 14.0)
                    .corner_radius(10.0)
                    .test_id("a-12")
                    .on_click(|| {}),
                    text("Engine")
                    .font_size(15.0)
                    .font_weight(Weight::Medium)
                    .foreground_color(Color::rgb(201, 201, 207))
                    .line_height(23.25)
                    .padding_edge(Edge::Top, 10.0)
                    .padding_edge(Edge::Trailing, 14.0)
                    .padding_edge(Edge::Bottom, 10.0)
                    .padding_edge(Edge::Leading, 14.0)
                    .corner_radius(10.0)
                    .test_id("a-13")
                    .on_click(|| {}),
                    text("For products")
                    .font_size(15.0)
                    .font_weight(Weight::Medium)
                    .foreground_color(Color::rgb(201, 201, 207))
                    .line_height(23.25)
                    .padding_edge(Edge::Top, 10.0)
                    .padding_edge(Edge::Trailing, 14.0)
                    .padding_edge(Edge::Bottom, 10.0)
                    .padding_edge(Edge::Leading, 14.0)
                    .corner_radius(10.0)
                    .test_id("a-14")
                    .on_click(|| {}),
                    text("Download")
                    .font_size(15.0)
                    .font_weight(Weight::Semibold)
                    .foreground_color(Color::rgb(16, 16, 14))
                    .line_height(23.25)
                    .padding_edge(Edge::Top, 11.0)
                    .padding_edge(Edge::Trailing, 18.0)
                    .padding_edge(Edge::Bottom, 11.0)
                    .padding_edge(Edge::Leading, 18.0)
                    .background_color(Color::rgb(240, 106, 76))
                    .corner_radius(22.625)
                    .test_id("a-15")
                    .on_click(|| {})
                    .padding_edge(Edge::Leading, 10.0),
                )
                .spacing(4.0)
                .alignment(VerticalAlignment::Center)
                .wrapping()
                .line_spacing(4.0)
                .test_id("div-10"),
            )
            .spacing(16.0)
            .alignment(VerticalAlignment::Center)
            .wrapping()
            .line_spacing(16.0)
            .padding_edge(Edge::Top, 22.0)
            .padding_edge(Edge::Trailing, 32.0)
            .padding_edge(Edge::Bottom, 22.0)
            .padding_edge(Edge::Leading, 32.0)
            .frame_max(1200.0, f64::MAX, Alignment::Leading)
            .test_id("nav-7")
            .padding_edge(Edge::Leading, 88.0)
            .padding_edge(Edge::Trailing, 88.0),
            vstack!(
                hstack!(
                    text("FREE")
                    .font_size(12.0)
                    .font_weight(Weight::Semibold)
                    .foreground_color(Color::rgb(16, 16, 14))
                    .tracking(0.24)
                    .line_height(18.6)
                    .multiline_text_alignment(TextAlignment::Center)
                    .padding_edge(Edge::Top, 3.0)
                    .padding_edge(Edge::Trailing, 9.0)
                    .padding_edge(Edge::Bottom, 3.0)
                    .padding_edge(Edge::Leading, 9.0)
                    .background_color(Color::rgb(240, 106, 76))
                    .corner_radius(12.2969)
                    .test_id("span-18"),
                    text("For macOS, Windows, Linux — and right here on the web")
                    .font_size(14.0)
                    .foreground_color(Color::rgb(214, 214, 219))
                    .line_height(21.7)
                    .multiline_text_alignment(TextAlignment::Center),
                )
                .spacing(10.0)
                .alignment(VerticalAlignment::Center)
                .padding_edge(Edge::Top, 8.0)
                .padding_edge(Edge::Trailing, 15.0)
                .padding_edge(Edge::Bottom, 8.0)
                .padding_edge(Edge::Leading, 9.0)
                .background_color(Color::rgba(255, 255, 255, 15))
                .corner_radius(20.2969)
                .border(Color::rgba(255, 255, 255, 31), 1.0)
                .test_id("p-17"),
                vstack!(
                    text("The web,")
                    .font_size(96.0)
                    .font_weight(Weight::Semibold)
                    .foreground_color(Color::rgb(244, 242, 236))
                    .tracking(-3.84)
                    .line_height(94.08)
                    .multiline_text_alignment(TextAlignment::Center)
                    .frame_width_aligned(492.2344, Alignment::Center),
                    spacer().frame(0.0, 116.0)
                    .test_id("br-20")
                    .padding_edge(Edge::Leading, 431.625),
                    text("hand-made.")
                    .font_size(96.0)
                    .font_weight(Weight::Semibold)
                    .foreground_color(Color::rgb(244, 242, 236))
                    .tracking(-3.84)
                    .line_height(94.08)
                    .multiline_text_alignment(TextAlignment::Center)
                    .frame_width_aligned(492.2344, Alignment::Center),
                )
                .alignment(HorizontalAlignment::Leading)
                .test_id("h1-19"),
                text("hop is a calm, refined browser that keeps out of your way. Don’t take our word for it — it’s running just below, inside this page.")
                .font_size(20.0)
                .foreground_color(Color::rgb(169, 169, 177))
                .line_height(31.0)
                .multiline_text_alignment(TextAlignment::Center)
                .test_id("p-21"),
                hstack!(
                    hstack!(
                        text("Download hop")
                        .font_weight(Weight::Semibold)
                        .foreground_color(Color::rgb(16, 16, 14))
                        .line_height(24.8)
                        .multiline_text_alignment(TextAlignment::Center),
                    )
                    .spacing(10.0)
                    .alignment(VerticalAlignment::Center)
                    .padding_edge(Edge::Trailing, 24.0)
                    .padding_edge(Edge::Leading, 24.0)
                    .frame_height_aligned(50.0, Alignment::Leading)
                    .background_color(Color::rgb(240, 106, 76))
                    .corner_radius(25.0)
                    .test_id("a-23")
                    .on_click(|| {}),
                    hstack!(
                        text("Try it first")
                        .font_weight(Weight::Medium)
                        .foreground_color(Color::rgb(236, 236, 239))
                        .line_height(24.8)
                        .multiline_text_alignment(TextAlignment::Center),
                        spacer()
                        .frame_aligned(16.0, 16.0, Alignment::Top)
                        .clipped()
                        .test_id("svg-25")
                        .on_click(|| {}),
                    )
                    .spacing(10.0)
                    .alignment(VerticalAlignment::Center)
                    .padding_edge(Edge::Top, 1.0)
                    .padding_edge(Edge::Trailing, 23.0)
                    .padding_edge(Edge::Bottom, 1.0)
                    .padding_edge(Edge::Leading, 23.0)
                    .frame_height_aligned(50.0, Alignment::Leading)
                    .background_color(Color::rgba(255, 255, 255, 15))
                    .corner_radius(25.0)
                    .border(Color::rgba(255, 255, 255, 46), 1.0)
                    .test_id("a-24")
                    .on_click(|| {}),
                )
                .spacing(12.0)
                .alignment(VerticalAlignment::Top)
                .wrapping()
                .line_spacing(12.0)
                .test_id("div-22")
                .padding_edge(Edge::Top, 6.0),
            )
            .spacing(26.0)
            .alignment(HorizontalAlignment::Center)
            .padding_edge(Edge::Top, 84.0)
            .padding_edge(Edge::Trailing, 32.0)
            .padding_edge(Edge::Leading, 32.0)
            .frame_max(980.0, f64::MAX, Alignment::Top)
            .test_id("div-16")
            .padding_edge(Edge::Leading, 198.0)
            .padding_edge(Edge::Trailing, 198.0),
            vstack!(
                vstack!(
                    hstack!(
                        hstack!(
                            spacer()
                            .frame_aligned(12.0, 12.0, Alignment::TopLeading)
                            .background_color(Color::rgb(255, 95, 87))
                            .corner_radius(6.0)
                            .test_id("span-30"),
                            spacer()
                            .frame_aligned(12.0, 12.0, Alignment::TopLeading)
                            .background_color(Color::rgb(254, 188, 46))
                            .corner_radius(6.0)
                            .test_id("span-31"),
                            spacer()
                            .frame_aligned(12.0, 12.0, Alignment::TopLeading)
                            .background_color(Color::rgb(40, 200, 64))
                            .corner_radius(6.0)
                            .test_id("span-32"),
                        )
                        .spacing(8.0)
                        .alignment(VerticalAlignment::Top)
                        .padding_edge(Edge::Trailing, 6.0)
                        .padding_edge(Edge::Leading, 2.0)
                        .test_id("div-29"),
                        hstack!(
                            spacer()
                            .frame_aligned(18.0, 18.0, Alignment::Top)
                            .clipped()
                            .test_id("svg-34")
                            .on_click(|| {}),
                            spacer()
                            .frame_aligned(12.0, 12.0, Alignment::Top)
                            .clipped()
                            .test_id("svg-35")
                            .on_click(|| {}),
                        )
                        .spacing(6.0)
                        .alignment(VerticalAlignment::Center)
                        .padding_edge(Edge::Trailing, 10.0)
                        .padding_edge(Edge::Leading, 10.0)
                        .frame_height_aligned(34.0, Alignment::Leading)
                        .background_color(Color::rgb(46, 46, 49))
                        .corner_radius(10.0)
                        .test_id("button-33")
                        .on_click(|| {}),
                        hstack!(
                            spacer()
                            .frame_aligned(18.0, 18.0, Alignment::Top)
                            .clipped()
                            .test_id("svg-37")
                            .on_click(|| {}),
                        )
                        .alignment(VerticalAlignment::Center)
                        .padding_edge(Edge::Top, 1.0)
                        .padding_edge(Edge::Trailing, 6.0)
                        .padding_edge(Edge::Bottom, 1.0)
                        .padding_edge(Edge::Leading, 6.0)
                        .frame_aligned(38.0, 34.0, Alignment::Center)
                        .background_color(Color::rgb(46, 46, 49))
                        .corner_radius(10.0)
                        .test_id("button-36")
                        .on_click(|| {}),
                        hstack!(
                            spacer()
                            .frame_aligned(16.0, 16.0, Alignment::TopLeading)
                            .clipped()
                            .test_id("svg-39"),
                            text_field("Search or enter website name", State::new(String::from("")).binding())
                            .bare()
                            .content_inset(Edges { top: 1.0, trailing: 2.0, bottom: 1.0, leading: 2.0 })
                            .frame_height_aligned(36.0, Alignment::TopLeading)
                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                            .clipped()
                            .test_id("input.hop-addr-41"),
                        )
                        .spacing(10.0)
                        .alignment(VerticalAlignment::Center)
                        .padding_edge(Edge::Trailing, 12.0)
                        .padding_edge(Edge::Leading, 12.0)
                        .frame_height_aligned(34.0, Alignment::Leading)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading)
                        .background_color(Color::rgb(46, 46, 49))
                        .corner_radius(10.0)
                        .overlay(UnitPoint::TOP_LEADING, text("Address")
                            .foreground_color(Color::rgb(154, 154, 161))
                            .line_height(24.8)
                            .frame_aligned(1.0, 1.0, Alignment::TopLeading)
                            .clipped()
                            .test_id("span.hop-sr-40")
                            .offset(11.0, 16.5))
                        .test_id("label-38"),
                        hstack!(
                            hstack!(
                                spacer()
                                .frame_aligned(17.0, 17.0, Alignment::Top)
                                .clipped()
                                .test_id("svg-44")
                                .on_click(|| {}),
                            )
                            .alignment(VerticalAlignment::Center)
                            .padding_edge(Edge::Top, 1.0)
                            .padding_edge(Edge::Trailing, 6.0)
                            .padding_edge(Edge::Bottom, 1.0)
                            .padding_edge(Edge::Leading, 6.0)
                            .frame_aligned(38.0, 34.0, Alignment::Center)
                            .background_color(Color::rgb(46, 46, 49))
                            .corner_radius(10.0)
                            .test_id("button-43")
                            .on_click(|| {}),
                            hstack!(
                                spacer()
                                .frame_aligned(17.0, 17.0, Alignment::Top)
                                .clipped()
                                .test_id("svg-46")
                                .on_click(|| {}),
                            )
                            .alignment(VerticalAlignment::Center)
                            .padding_edge(Edge::Top, 1.0)
                            .padding_edge(Edge::Trailing, 6.0)
                            .padding_edge(Edge::Bottom, 1.0)
                            .padding_edge(Edge::Leading, 6.0)
                            .frame_aligned(38.0, 34.0, Alignment::Center)
                            .background_color(Color::rgb(46, 46, 49))
                            .corner_radius(10.0)
                            .test_id("button-45")
                            .on_click(|| {}),
                            hstack!(
                                spacer()
                                .frame_aligned(17.0, 17.0, Alignment::Top)
                                .clipped()
                                .test_id("svg-48")
                                .on_click(|| {}),
                            )
                            .alignment(VerticalAlignment::Center)
                            .padding_edge(Edge::Top, 1.0)
                            .padding_edge(Edge::Trailing, 6.0)
                            .padding_edge(Edge::Bottom, 1.0)
                            .padding_edge(Edge::Leading, 6.0)
                            .frame_aligned(38.0, 34.0, Alignment::Center)
                            .background_color(Color::rgb(46, 46, 49))
                            .corner_radius(10.0)
                            .test_id("button-47")
                            .on_click(|| {}),
                        )
                        .spacing(6.0)
                        .alignment(VerticalAlignment::Top)
                        .test_id("div-42"),
                    )
                    .spacing(10.0)
                    .alignment(VerticalAlignment::Center)
                    .wrapping()
                    .line_spacing(10.0)
                    .padding_edge(Edge::Top, 8.0)
                    .padding_edge(Edge::Trailing, 14.0)
                    .padding_edge(Edge::Bottom, 9.0)
                    .padding_edge(Edge::Leading, 14.0)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading)
                    .frame_min(0.0, 71.0, Alignment::Leading)
                    .background_color(Color::rgb(30, 30, 32))
                    .overlay(UnitPoint::BOTTOM, spacer().frame_height(1.0).background_color(Color::rgba(255, 255, 255, 20)))
                    .test_id("div-28"),
                    spacer()
                    .frame_height_aligned(660.0, Alignment::TopLeading)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .clipped()
                    .overlay(UnitPoint::TOP_LEADING, spacer()
                        .frame_aligned(1262.0, 660.0, Alignment::TopLeading)
                        .background_gradient(Gradient::linear(Color::rgb(14, 18, 28), Color::rgb(30, 39, 56)).direction(UnitPoint { x: 0.5, y: 0.0 }, UnitPoint { x: 0.5, y: 1.0 }))
                        .overlay(UnitPoint::TOP_LEADING, spacer()
                            .frame_aligned(1262.0, 520.0, Alignment::TopLeading)
                            .clipped()
                            .test_id("svg-51"))
                        .overlay(UnitPoint::TOP_LEADING, spacer()
                            .frame_aligned(1262.0, 240.0, Alignment::TopLeading)
                            .clipped()
                            .test_id("svg-52")
                            .offset(0.0, 420.0))
                        .overlay(UnitPoint::TOP_LEADING, vstack!(
                                image(picture_hop_logo_light_0())
                                .resizable()
                                .frame_height_aligned(40.0, Alignment::TopLeading)
                                .clipped()
                                .test_id("img-55")
                                .padding_edge(Edge::Bottom, 4.0)
                                .frame_max(f64::INFINITY, f64::MAX, Alignment::Top),
                                vstack!(
                                    text("Favorites")
                                    .font_size(17.0)
                                    .font_weight(Weight::Semibold)
                                    .foreground_color(Color::rgb(242, 242, 245))
                                    .line_height(26.35)
                                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                    .test_id("h2-57"),
                                    hstack!(
                                        vstack!(
                                            hstack!(
                                                text("D")
                                                .font_size(28.0)
                                                .font_weight(Weight::Semibold)
                                                .foreground_color(Color::rgb(244, 242, 236))
                                                .multiline_text_alignment(TextAlignment::Center),
                                            )
                                            .alignment(VerticalAlignment::Center)
                                            .padding_length(1.0)
                                            .frame_aligned(64.0, 64.0, Alignment::Center)
                                            .background_color(Color::rgb(23, 21, 26))
                                            .corner_radius(16.0)
                                            .border(Color::rgba(255, 255, 255, 26), 1.0)
                                            .test_id("span-60")
                                            .on_click(|| {}),
                                            text("The demo page")
                                            .font_size(13.0)
                                            .foreground_color(Color::rgb(230, 230, 234))
                                            .line_height(16.9)
                                            .multiline_text_alignment(TextAlignment::Center)
                                            .test_id("span-61")
                                            .on_click(|| {}),
                                        )
                                        .spacing(9.0)
                                        .alignment(HorizontalAlignment::Center)
                                        .padding_edge(Edge::Top, 6.0)
                                        .padding_edge(Edge::Bottom, 6.0)
                                        .frame_aligned(104.0, 101.8984, Alignment::Top)
                                        .test_id("button-59")
                                        .on_click(|| {}),
                                        vstack!(
                                            hstack!(
                                                text("L")
                                                .font_size(28.0)
                                                .font_weight(Weight::Semibold)
                                                .foreground_color(Color::rgb(244, 242, 236))
                                                .multiline_text_alignment(TextAlignment::Center),
                                            )
                                            .alignment(VerticalAlignment::Center)
                                            .padding_length(1.0)
                                            .frame_aligned(64.0, 64.0, Alignment::Center)
                                            .background_color(Color::rgb(23, 21, 26))
                                            .corner_radius(16.0)
                                            .border(Color::rgba(255, 255, 255, 26), 1.0)
                                            .test_id("span-63")
                                            .on_click(|| {}),
                                            text("A long page")
                                            .font_size(13.0)
                                            .foreground_color(Color::rgb(230, 230, 234))
                                            .line_height(16.9)
                                            .multiline_text_alignment(TextAlignment::Center)
                                            .test_id("span-64")
                                            .on_click(|| {}),
                                        )
                                        .spacing(9.0)
                                        .alignment(HorizontalAlignment::Center)
                                        .padding_edge(Edge::Top, 6.0)
                                        .padding_edge(Edge::Bottom, 6.0)
                                        .frame_aligned(104.0, 101.8984, Alignment::Top)
                                        .test_id("button-62")
                                        .on_click(|| {}),
                                        vstack!(
                                            hstack!(
                                                text("E")
                                                .font_size(28.0)
                                                .font_weight(Weight::Semibold)
                                                .foreground_color(Color::rgb(244, 242, 236))
                                                .multiline_text_alignment(TextAlignment::Center),
                                            )
                                            .alignment(VerticalAlignment::Center)
                                            .padding_length(1.0)
                                            .frame_aligned(64.0, 64.0, Alignment::Center)
                                            .background_color(Color::rgb(23, 21, 26))
                                            .corner_radius(16.0)
                                            .border(Color::rgba(255, 255, 255, 26), 1.0)
                                            .test_id("span-66")
                                            .on_click(|| {}),
                                            text("example.com")
                                            .font_size(13.0)
                                            .foreground_color(Color::rgb(230, 230, 234))
                                            .line_height(16.9)
                                            .multiline_text_alignment(TextAlignment::Center)
                                            .test_id("span-67")
                                            .on_click(|| {}),
                                        )
                                        .spacing(9.0)
                                        .alignment(HorizontalAlignment::Center)
                                        .padding_edge(Edge::Top, 6.0)
                                        .padding_edge(Edge::Bottom, 6.0)
                                        .frame_aligned(104.0, 101.8984, Alignment::Top)
                                        .test_id("button-65")
                                        .on_click(|| {}),
                                        vstack!(
                                            hstack!(
                                                image(picture_hop_icon_ink_1())
                                                .resizable()
                                                .frame_aligned(44.0, 44.0, Alignment::Top)
                                                .clipped()
                                                .test_id("img-70")
                                                .on_click(|| {}),
                                            )
                                            .alignment(VerticalAlignment::Center)
                                            .frame_aligned(62.0, 62.0, Alignment::Center)
                                            .background_color(Color::rgb(240, 106, 76))
                                            .corner_radius(16.0)
                                            .test_id("span-69")
                                            .on_click(|| {}),
                                            text("hop on the web")
                                            .font_size(13.0)
                                            .foreground_color(Color::rgb(230, 230, 234))
                                            .line_height(16.9)
                                            .multiline_text_alignment(TextAlignment::Center)
                                            .test_id("span-71")
                                            .on_click(|| {}),
                                        )
                                        .spacing(9.0)
                                        .alignment(HorizontalAlignment::Center)
                                        .padding_edge(Edge::Top, 6.0)
                                        .padding_edge(Edge::Bottom, 6.0)
                                        .frame_aligned(104.0, 101.8984, Alignment::Top)
                                        .test_id("button-68")
                                        .on_click(|| {}),
                                    )
                                    .spacing(18.0)
                                    .alignment(VerticalAlignment::Top)
                                    .wrapping()
                                    .line_spacing(18.0)
                                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                    .test_id("div-58")
                                    .padding_edge(Edge::Top, 14.0),
                                )
                                .alignment(HorizontalAlignment::Leading)
                                .padding_edge(Edge::Top, 21.0)
                                .padding_edge(Edge::Trailing, 25.0)
                                .padding_edge(Edge::Bottom, 23.0)
                                .padding_edge(Edge::Leading, 25.0)
                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                .background_color(Color::rgba(12, 15, 24, 107))
                                .corner_radius(22.0)
                                .border(Color::rgba(255, 255, 255, 23), 1.0)
                                .test_id("section-56"),
                                vstack!(
                                    text("Continue Where You Left Off")
                                    .font_size(17.0)
                                    .font_weight(Weight::Semibold)
                                    .foreground_color(Color::rgb(242, 242, 245))
                                    .line_height(26.35)
                                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                    .test_id("h2-73"),
                                    hstack!(
                                        vstack!(
                                            vstack!(
                                                spacer()
                                                .frame_aligned(110.7031, 10.0, Alignment::TopLeading)
                                                .background_color(Color::rgb(43, 42, 40))
                                                .corner_radius(3.0)
                                                .test_id("span-77")
                                                .on_click(|| {}),
                                                spacer()
                                                .frame_aligned(192.5313, 5.0, Alignment::TopLeading)
                                                .background_color(Color::rgb(207, 203, 196))
                                                .corner_radius(2.5)
                                                .test_id("span-78")
                                                .on_click(|| {}),
                                                spacer()
                                                .frame_aligned(154.0234, 5.0, Alignment::TopLeading)
                                                .background_color(Color::rgb(207, 203, 196))
                                                .corner_radius(2.5)
                                                .test_id("span-79")
                                                .on_click(|| {}),
                                                hstack!(
                                                    spacer()
                                                    .frame_aligned(22.0, 22.0, Alignment::TopLeading)
                                                    .background_color(Color::rgb(232, 89, 74))
                                                    .corner_radius(5.0)
                                                    .test_id("span-81")
                                                    .on_click(|| {}),
                                                    spacer()
                                                    .frame_aligned(22.0, 22.0, Alignment::TopLeading)
                                                    .background_color(Color::rgb(240, 164, 58))
                                                    .corner_radius(5.0)
                                                    .test_id("span-82")
                                                    .on_click(|| {}),
                                                    spacer()
                                                    .frame_aligned(22.0, 22.0, Alignment::TopLeading)
                                                    .background_color(Color::rgb(244, 211, 94))
                                                    .corner_radius(5.0)
                                                    .test_id("span-83")
                                                    .on_click(|| {}),
                                                    spacer()
                                                    .frame_aligned(22.0, 22.0, Alignment::TopLeading)
                                                    .background_color(Color::rgb(91, 180, 124))
                                                    .corner_radius(5.0)
                                                    .test_id("span-84")
                                                    .on_click(|| {}),
                                                    spacer()
                                                    .frame_aligned(22.0, 22.0, Alignment::TopLeading)
                                                    .background_color(Color::rgb(79, 102, 209))
                                                    .corner_radius(5.0)
                                                    .test_id("span-85")
                                                    .on_click(|| {}),
                                                )
                                                .spacing(5.0)
                                                .alignment(VerticalAlignment::Top)
                                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                                .test_id("span-80")
                                                .on_click(|| {})
                                                .padding_edge(Edge::Top, 6.0),
                                            )
                                            .spacing(7.0)
                                            .alignment(HorizontalAlignment::Leading)
                                            .padding_edge(Edge::Top, 14.0)
                                            .padding_edge(Edge::Trailing, 16.0)
                                            .padding_edge(Edge::Bottom, 14.0)
                                            .padding_edge(Edge::Leading, 16.0)
                                            .frame_height_aligned(118.0, Alignment::TopLeading)
                                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                            .background_color(Color::rgb(246, 244, 240))
                                            .corner_radius(12.0)
                                            .clipped()
                                            .test_id("span-76")
                                            .on_click(|| {}),
                                            vstack!(
                                                text("The demo page")
                                                .font_size(15.0)
                                                .font_weight(Weight::Semibold)
                                                .foreground_color(Color::rgb(242, 242, 245))
                                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                                .test_id("span-87")
                                                .on_click(|| {}),
                                                text("In another window")
                                                .font_size(13.0)
                                                .foreground_color(Color::rgb(154, 154, 161))
                                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                                .test_id("span-88")
                                                .on_click(|| {}),
                                            )
                                            .spacing(1.0)
                                            .alignment(HorizontalAlignment::Leading)
                                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                            .test_id("span-86")
                                            .on_click(|| {}),
                                        )
                                        .spacing(10.0)
                                        .alignment(HorizontalAlignment::Leading)
                                        .frame_aligned(272.6641, 162.5, Alignment::TopLeading)
                                        .test_id("button-75")
                                        .on_click(|| {}),
                                        vstack!(
                                            vstack!(
                                                spacer()
                                                .frame_aligned(91.4453, 10.0, Alignment::TopLeading)
                                                .background_color(Color::rgb(43, 42, 40))
                                                .corner_radius(3.0)
                                                .test_id("span-91")
                                                .on_click(|| {}),
                                                spacer()
                                                .frame_aligned(221.4063, 5.0, Alignment::TopLeading)
                                                .background_color(Color::rgb(218, 215, 209))
                                                .corner_radius(2.5)
                                                .test_id("span-92")
                                                .on_click(|| {}),
                                                spacer()
                                                .frame_aligned(206.9688, 5.0, Alignment::TopLeading)
                                                .background_color(Color::rgb(218, 215, 209))
                                                .corner_radius(2.5)
                                                .test_id("span-93")
                                                .on_click(|| {}),
                                                spacer()
                                                .frame_aligned(216.5938, 5.0, Alignment::TopLeading)
                                                .background_color(Color::rgb(218, 215, 209))
                                                .corner_radius(2.5)
                                                .test_id("span-94")
                                                .on_click(|| {}),
                                                spacer()
                                                .frame_aligned(168.4609, 5.0, Alignment::TopLeading)
                                                .background_color(Color::rgb(218, 215, 209))
                                                .corner_radius(2.5)
                                                .test_id("span-95")
                                                .on_click(|| {}),
                                                spacer()
                                                .frame_aligned(211.7813, 5.0, Alignment::TopLeading)
                                                .background_color(Color::rgb(218, 215, 209))
                                                .corner_radius(2.5)
                                                .test_id("span-96")
                                                .on_click(|| {}),
                                                spacer()
                                                .frame_aligned(192.5313, 5.0, Alignment::TopLeading)
                                                .background_color(Color::rgb(218, 215, 209))
                                                .corner_radius(2.5)
                                                .test_id("span-97")
                                                .on_click(|| {}),
                                            )
                                            .spacing(6.0)
                                            .alignment(HorizontalAlignment::Leading)
                                            .padding_edge(Edge::Top, 14.0)
                                            .padding_edge(Edge::Trailing, 16.0)
                                            .padding_edge(Edge::Bottom, 14.0)
                                            .padding_edge(Edge::Leading, 16.0)
                                            .frame_height_aligned(118.0, Alignment::TopLeading)
                                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                            .background_color(Color::rgb(255, 255, 255))
                                            .corner_radius(12.0)
                                            .clipped()
                                            .test_id("span-90")
                                            .on_click(|| {}),
                                            vstack!(
                                                text("A long page")
                                                .font_size(15.0)
                                                .font_weight(Weight::Semibold)
                                                .foreground_color(Color::rgb(242, 242, 245))
                                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                                .test_id("span-99")
                                                .on_click(|| {}),
                                                text("Closed")
                                                .font_size(13.0)
                                                .foreground_color(Color::rgb(154, 154, 161))
                                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                                .test_id("span-100")
                                                .on_click(|| {}),
                                            )
                                            .spacing(1.0)
                                            .alignment(HorizontalAlignment::Leading)
                                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                            .test_id("span-98")
                                            .on_click(|| {}),
                                        )
                                        .spacing(10.0)
                                        .alignment(HorizontalAlignment::Leading)
                                        .frame_aligned(272.6641, 162.5, Alignment::TopLeading)
                                        .test_id("button-89")
                                        .on_click(|| {})
                                        .padding_edge(Edge::Leading, 16.0),
                                        vstack!(
                                            hstack!(
                                                vstack!(
                                                    spacer()
                                                    .frame_aligned(98.1563, 8.0, Alignment::TopLeading)
                                                    .background_color(Color::rgb(58, 58, 64))
                                                    .corner_radius(3.0)
                                                    .test_id("span-104")
                                                    .on_click(|| {}),
                                                    spacer()
                                                    .frame_aligned(163.5938, 4.0, Alignment::TopLeading)
                                                    .background_color(Color::rgb(211, 211, 217))
                                                    .corner_radius(2.0)
                                                    .test_id("span-105")
                                                    .on_click(|| {}),
                                                    spacer()
                                                    .frame_aligned(130.875, 4.0, Alignment::TopLeading)
                                                    .background_color(Color::rgb(211, 211, 217))
                                                    .corner_radius(2.0)
                                                    .test_id("span-106")
                                                    .on_click(|| {}),
                                                )
                                                .spacing(6.0)
                                                .alignment(HorizontalAlignment::Leading)
                                                .padding_length(12.0)
                                                .frame_width_aligned(187.5938, Alignment::TopLeading)
                                                .background_color(Color::rgb(253, 253, 255))
                                                .corner_radius(8.0)
                                                .test_id("span-103")
                                                .on_click(|| {}),
                                            )
                                            .alignment(VerticalAlignment::Center)
                                            .frame_height_aligned(118.0, Alignment::Center)
                                            .frame_max(f64::INFINITY, f64::MAX, Alignment::Center)
                                            .background_color(Color::rgb(238, 238, 241))
                                            .corner_radius(12.0)
                                            .clipped()
                                            .test_id("span-102")
                                            .on_click(|| {}),
                                            vstack!(
                                                text("example.com")
                                                .font_size(15.0)
                                                .font_weight(Weight::Semibold)
                                                .foreground_color(Color::rgb(242, 242, 245))
                                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                                .test_id("span-108")
                                                .on_click(|| {}),
                                                text("In another window")
                                                .font_size(13.0)
                                                .foreground_color(Color::rgb(154, 154, 161))
                                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                                .test_id("span-109")
                                                .on_click(|| {}),
                                            )
                                            .spacing(1.0)
                                            .alignment(HorizontalAlignment::Leading)
                                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                            .test_id("span-107")
                                            .on_click(|| {}),
                                        )
                                        .spacing(10.0)
                                        .alignment(HorizontalAlignment::Leading)
                                        .frame_aligned(272.6641, 162.5, Alignment::TopLeading)
                                        .test_id("button-101")
                                        .on_click(|| {})
                                        .padding_edge(Edge::Leading, 16.0),
                                    )
                                    .alignment(VerticalAlignment::Top)
                                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                    .test_id("div-74")
                                    .padding_edge(Edge::Top, 14.0),
                                )
                                .alignment(HorizontalAlignment::Leading)
                                .padding_edge(Edge::Top, 21.0)
                                .padding_edge(Edge::Trailing, 25.0)
                                .padding_edge(Edge::Bottom, 23.0)
                                .padding_edge(Edge::Leading, 25.0)
                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                .background_color(Color::rgba(12, 15, 24, 107))
                                .corner_radius(22.0)
                                .border(Color::rgba(255, 255, 255, 23), 1.0)
                                .test_id("section-72"),
                                vstack!(
                                    text("Frequently Visited")
                                    .font_size(17.0)
                                    .font_weight(Weight::Semibold)
                                    .foreground_color(Color::rgb(242, 242, 245))
                                    .line_height(26.35)
                                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                    .test_id("h2-111"),
                                    hstack!(
                                        vstack!(
                                            hstack!(
                                                hstack!(
                                                    text("D")
                                                    .font_size(22.0)
                                                    .font_weight(Weight::Bold)
                                                    .foreground_color(Color::rgb(255, 255, 255)),
                                                )
                                                .alignment(VerticalAlignment::Center)
                                                .frame_aligned(44.0, 44.0, Alignment::Center)
                                                .background_color(Color::rgba(0, 0, 0, 71))
                                                .corner_radius(12.0)
                                                .test_id("span-115")
                                                .on_click(|| {}),
                                            )
                                            .alignment(VerticalAlignment::Center)
                                            .frame_height_aligned(92.0, Alignment::Center)
                                            .frame_max(f64::INFINITY, f64::MAX, Alignment::Center)
                                            .background_gradient(Gradient::linear(Color::rgb(232, 128, 106), Color::rgb(201, 88, 74)).direction(UnitPoint { x: 0.3678, y: -0.2917 }, UnitPoint { x: 0.6322, y: 1.2917 }))
                                            .corner_radius(12.0)
                                            .test_id("span-114")
                                            .on_click(|| {}),
                                            vstack!(
                                                text("The demo page")
                                                .font_size(15.0)
                                                .font_weight(Weight::Semibold)
                                                .foreground_color(Color::rgb(242, 242, 245))
                                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                                .test_id("span-117")
                                                .on_click(|| {}),
                                                text("/pages/demo.html")
                                                .font_size(13.0)
                                                .foreground_color(Color::rgb(154, 154, 161))
                                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                                .test_id("span-118")
                                                .on_click(|| {}),
                                            )
                                            .spacing(1.0)
                                            .alignment(HorizontalAlignment::Leading)
                                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                            .test_id("span-116")
                                            .on_click(|| {}),
                                        )
                                        .spacing(10.0)
                                        .alignment(HorizontalAlignment::Leading)
                                        .frame_aligned(200.5, 136.5, Alignment::TopLeading)
                                        .test_id("button-113")
                                        .on_click(|| {}),
                                        vstack!(
                                            hstack!(
                                                hstack!(
                                                    text("L")
                                                    .font_size(22.0)
                                                    .font_weight(Weight::Bold)
                                                    .foreground_color(Color::rgb(255, 255, 255)),
                                                )
                                                .alignment(VerticalAlignment::Center)
                                                .frame_aligned(44.0, 44.0, Alignment::Center)
                                                .background_color(Color::rgba(0, 0, 0, 71))
                                                .corner_radius(12.0)
                                                .test_id("span-121")
                                                .on_click(|| {}),
                                            )
                                            .alignment(VerticalAlignment::Center)
                                            .frame_height_aligned(92.0, Alignment::Center)
                                            .frame_max(f64::INFINITY, f64::MAX, Alignment::Center)
                                            .background_gradient(Gradient::linear(Color::rgb(127, 144, 214), Color::rgb(85, 104, 181)).direction(UnitPoint { x: 0.3678, y: -0.2917 }, UnitPoint { x: 0.6322, y: 1.2917 }))
                                            .corner_radius(12.0)
                                            .test_id("span-120")
                                            .on_click(|| {}),
                                            vstack!(
                                                text("A long page")
                                                .font_size(15.0)
                                                .font_weight(Weight::Semibold)
                                                .foreground_color(Color::rgb(242, 242, 245))
                                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                                .test_id("span-123")
                                                .on_click(|| {}),
                                                text("/pages/long.html")
                                                .font_size(13.0)
                                                .foreground_color(Color::rgb(154, 154, 161))
                                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                                .test_id("span-124")
                                                .on_click(|| {}),
                                            )
                                            .spacing(1.0)
                                            .alignment(HorizontalAlignment::Leading)
                                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                            .test_id("span-122")
                                            .on_click(|| {}),
                                        )
                                        .spacing(10.0)
                                        .alignment(HorizontalAlignment::Leading)
                                        .frame_aligned(200.5, 136.5, Alignment::TopLeading)
                                        .test_id("button-119")
                                        .on_click(|| {})
                                        .padding_edge(Edge::Leading, 16.0),
                                        vstack!(
                                            hstack!(
                                                hstack!(
                                                    text("E")
                                                    .font_size(22.0)
                                                    .font_weight(Weight::Bold)
                                                    .foreground_color(Color::rgb(255, 255, 255)),
                                                )
                                                .alignment(VerticalAlignment::Center)
                                                .frame_aligned(44.0, 44.0, Alignment::Center)
                                                .background_color(Color::rgba(0, 0, 0, 71))
                                                .corner_radius(12.0)
                                                .test_id("span-127")
                                                .on_click(|| {}),
                                            )
                                            .alignment(VerticalAlignment::Center)
                                            .frame_height_aligned(92.0, Alignment::Center)
                                            .frame_max(f64::INFINITY, f64::MAX, Alignment::Center)
                                            .background_gradient(Gradient::linear(Color::rgb(115, 176, 142), Color::rgb(74, 136, 101)).direction(UnitPoint { x: 0.3678, y: -0.2917 }, UnitPoint { x: 0.6322, y: 1.2917 }))
                                            .corner_radius(12.0)
                                            .test_id("span-126")
                                            .on_click(|| {}),
                                            vstack!(
                                                text("example.com")
                                                .font_size(15.0)
                                                .font_weight(Weight::Semibold)
                                                .foreground_color(Color::rgb(242, 242, 245))
                                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                                .test_id("span-129")
                                                .on_click(|| {}),
                                                text("example.com")
                                                .font_size(13.0)
                                                .foreground_color(Color::rgb(154, 154, 161))
                                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                                .test_id("span-130")
                                                .on_click(|| {}),
                                            )
                                            .spacing(1.0)
                                            .alignment(HorizontalAlignment::Leading)
                                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                            .test_id("span-128")
                                            .on_click(|| {}),
                                        )
                                        .spacing(10.0)
                                        .alignment(HorizontalAlignment::Leading)
                                        .frame_aligned(200.5, 136.5, Alignment::TopLeading)
                                        .test_id("button-125")
                                        .on_click(|| {})
                                        .padding_edge(Edge::Leading, 16.0),
                                        vstack!(
                                            hstack!(
                                                hstack!(
                                                    text("T")
                                                    .font_size(22.0)
                                                    .font_weight(Weight::Bold)
                                                    .foreground_color(Color::rgb(255, 255, 255)),
                                                )
                                                .alignment(VerticalAlignment::Center)
                                                .frame_aligned(44.0, 44.0, Alignment::Center)
                                                .background_color(Color::rgba(0, 0, 0, 71))
                                                .corner_radius(12.0)
                                                .test_id("span-133")
                                                .on_click(|| {}),
                                            )
                                            .alignment(VerticalAlignment::Center)
                                            .frame_height_aligned(92.0, Alignment::Center)
                                            .frame_max(f64::INFINITY, f64::MAX, Alignment::Center)
                                            .background_gradient(Gradient::linear(Color::rgb(211, 166, 96), Color::rgb(169, 126, 53)).direction(UnitPoint { x: 0.3678, y: -0.2917 }, UnitPoint { x: 0.6322, y: 1.2917 }))
                                            .corner_radius(12.0)
                                            .test_id("span-132")
                                            .on_click(|| {}),
                                            vstack!(
                                                text("Threads")
                                                .font_size(15.0)
                                                .font_weight(Weight::Semibold)
                                                .foreground_color(Color::rgb(242, 242, 245))
                                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                                .test_id("span-135")
                                                .on_click(|| {}),
                                                text("/pages/threads.html")
                                                .font_size(13.0)
                                                .foreground_color(Color::rgb(154, 154, 161))
                                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                                .test_id("span-136")
                                                .on_click(|| {}),
                                            )
                                            .spacing(1.0)
                                            .alignment(HorizontalAlignment::Leading)
                                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                            .test_id("span-134")
                                            .on_click(|| {}),
                                        )
                                        .spacing(10.0)
                                        .alignment(HorizontalAlignment::Leading)
                                        .frame_aligned(200.5, 136.5, Alignment::TopLeading)
                                        .test_id("button-131")
                                        .on_click(|| {})
                                        .padding_edge(Edge::Leading, 16.0),
                                    )
                                    .alignment(VerticalAlignment::Top)
                                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                    .test_id("div-112")
                                    .padding_edge(Edge::Top, 14.0),
                                )
                                .alignment(HorizontalAlignment::Leading)
                                .padding_edge(Edge::Top, 21.0)
                                .padding_edge(Edge::Trailing, 25.0)
                                .padding_edge(Edge::Bottom, 23.0)
                                .padding_edge(Edge::Leading, 25.0)
                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                .background_color(Color::rgba(12, 15, 24, 107))
                                .corner_radius(22.0)
                                .border(Color::rgba(255, 255, 255, 23), 1.0)
                                .test_id("section-110"),
                            )
                            .spacing(22.0)
                            .alignment(HorizontalAlignment::Leading)
                            .padding_edge(Edge::Top, 36.0)
                            .padding_edge(Edge::Trailing, 28.0)
                            .padding_edge(Edge::Bottom, 48.0)
                            .padding_edge(Edge::Leading, 28.0)
                            .frame_max(900.0, f64::MAX, Alignment::TopLeading)
                            .test_id("div-54")
                            .padding_edge(Edge::Leading, 153.0)
                            .padding_edge(Edge::Trailing, 153.0)
                            .frame_aligned(1262.0, 660.0, Alignment::TopLeading)
                            .clipped()
                            .test_id("div-53"))
                        .test_id("div-50"))
                    .test_id("div-49"),
                )
                .alignment(HorizontalAlignment::Leading)
                .padding_length(1.0)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                .background_color(Color::rgb(30, 30, 32))
                .corner_radius(18.0)
                .border(Color::rgba(255, 255, 255, 31), 1.0)
                .shadow_color(120.0, Color::rgba(0, 0, 0, 179))
                .shadow_offset(0.0, 50.0)
                .clipped()
                .test_id("div-27"),
                hstack!(
                    hstack!(
                        spacer()
                        .frame_aligned(8.0, 8.0, Alignment::TopLeading)
                        .background_color(Color::rgb(107, 211, 139))
                        .corner_radius(4.0)
                        .shadow_color(1.0, Color::rgba(107, 211, 139, 46))
                        .test_id("span-139"),
                        text("Live — this is hop itself, compiled to WebAssembly and running in your tab.")
                        .font_size(14.0)
                        .foreground_color(Color::rgb(169, 169, 177))
                        .line_height(21.7),
                    )
                    .spacing(10.0)
                    .alignment(VerticalAlignment::Center)
                    .test_id("p-138"),
                    spacer(),
                    rich_text([
                        run("Try "),
                        run("example.com").foreground_color(Color::rgb(230, 230, 234)),
                        run(" or "),
                        run("/pages/demo.html").foreground_color(Color::rgb(230, 230, 234)),
                        run(" in the address bar."),
                    ])
                    .font_size(14.0)
                    .foreground_color(Color::rgb(169, 169, 177))
                    .line_height(21.7)
                    .test_id("p-140"),
                )
                .spacing(12.0)
                .alignment(VerticalAlignment::Center)
                .wrapping()
                .line_spacing(12.0)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading)
                .test_id("div-137")
                .padding_edge(Edge::Top, 18.0),
            )
            .alignment(HorizontalAlignment::Leading)
            .padding_edge(Edge::Top, 72.0)
            .padding_edge(Edge::Trailing, 32.0)
            .padding_edge(Edge::Leading, 32.0)
            .frame_max(1264.0, f64::MAX, Alignment::TopLeading)
            .test_id("section-26")
            .padding_edge(Edge::Leading, 56.0)
            .padding_edge(Edge::Trailing, 56.0),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding_edge(Edge::Bottom, 120.0)
        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
        .background_gradient(Gradient::linear(Color::rgb(14, 18, 28), Color::rgb(42, 54, 76)).direction(UnitPoint { x: 0.5, y: 0.0 }, UnitPoint { x: 0.5, y: 1.0 }).stop(0.58, Color::rgb(30, 39, 56)))
        .overlay(UnitPoint::TOP_LEADING, spacer()
            .frame_aligned(1440.0, 760.0, Alignment::TopLeading)
            .clipped()
            .test_id("svg-3"))
        .overlay(UnitPoint::TOP_LEADING, spacer()
            .frame_aligned(260.0, 260.0, Alignment::TopLeading)
            .background_gradient(Gradient::radial(Color::rgba(233, 228, 212, 56), Color::rgba(233, 228, 212, 0)).stop(0.4, Color::rgba(233, 228, 212, 15)).stop(0.7, Color::rgba(233, 228, 212, 0)))
            .corner_radius(130.0)
            .test_id("div-4")
            .offset(1050.7969, 92.0))
        .overlay(UnitPoint::TOP_LEADING, spacer()
            .frame_aligned(44.0, 44.0, Alignment::TopLeading)
            .background_color(Color::rgb(233, 228, 212))
            .corner_radius(22.0)
            .shadow_color(40.0, Color::rgba(233, 228, 212, 89))
            .test_id("div-5")
            .offset(1158.7969, 200.0))
        .overlay(UnitPoint::TOP_LEADING, spacer()
            .frame_aligned(1440.0, 360.0, Alignment::TopLeading)
            .clipped()
            .test_id("svg-6")
            .offset(0.0, 1233.6953))
        .test_id("header-2"),
        vstack!(
            vstack!(
                text("THE BROWSER")
                .font_size(13.0)
                .foreground_color(Color::rgb(240, 106, 76))
                .tracking(1.04)
                .line_height(20.15)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                .test_id("p-145"),
                text("Quiet by design.")
                .font_size(56.0)
                .font_weight(Weight::Semibold)
                .foreground_color(Color::rgb(244, 242, 236))
                .tracking(-1.96)
                .line_height(58.24)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                .test_id("h2-146"),
                text("One slim bar, a start page worth opening, and pages that keep to themselves. hop does less on screen so the web gets more of it.")
                .font_size(19.0)
                .foreground_color(Color::rgb(169, 169, 177))
                .line_height(29.45)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                .test_id("p-147"),
            )
            .spacing(16.0)
            .alignment(HorizontalAlignment::Leading)
            .frame_max(720.0, f64::MAX, Alignment::TopLeading)
            .test_id("div-144")
            .padding_edge(Edge::Trailing, 480.0),
            hstack!(
                vstack!(
                    vstack!(
                        hstack!(
                            vstack!(
                                spacer()
                                .frame_height_aligned(92.0, Alignment::TopLeading)
                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                .background_gradient(Gradient::linear(Color::rgb(246, 241, 231), Color::rgb(230, 228, 214)).direction(UnitPoint { x: 0.5, y: 0.0 }, UnitPoint { x: 0.5, y: 1.0 }))
                                .corner_radius(12.0)
                                .clipped()
                                .overlay(UnitPoint::TOP_LEADING, spacer()
                                    .frame_aligned(14.0, 14.0, Alignment::TopLeading)
                                    .background_color(Color::rgb(244, 207, 166))
                                    .corner_radius(7.0)
                                    .test_id("span-153")
                                    .offset(90.3594, 18.0))
                                .overlay(UnitPoint::TOP_LEADING, spacer()
                                    .frame_aligned(125.5, 92.0, Alignment::TopLeading)
                                    .clipped()
                                    .test_id("svg-154"))
                                .test_id("div-152"),
                                text("Hills")
                                .font_size(13.0)
                                .foreground_color(Color::rgb(169, 169, 177))
                                .line_height(20.15)
                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                .test_id("figcaption-155"),
                            )
                            .spacing(8.0)
                            .alignment(HorizontalAlignment::Leading)
                            .frame_aligned(125.5, 120.1484, Alignment::TopLeading)
                            .test_id("figure-151"),
                            vstack!(
                                spacer()
                                .frame_height_aligned(92.0, Alignment::TopLeading)
                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                .background_gradient(Gradient::linear(Color::rgb(250, 234, 218), Color::rgb(243, 207, 177)).direction(UnitPoint { x: 0.5, y: 0.0 }, UnitPoint { x: 0.5, y: 1.0 }))
                                .corner_radius(12.0)
                                .clipped()
                                .overlay(UnitPoint::TOP_LEADING, spacer()
                                    .frame_aligned(16.0, 16.0, Alignment::TopLeading)
                                    .background_color(Color::rgb(255, 244, 230))
                                    .corner_radius(8.0)
                                    .test_id("span-158")
                                    .offset(30.1172, 22.0))
                                .overlay(UnitPoint::TOP_LEADING, spacer()
                                    .frame_aligned(125.5, 92.0, Alignment::TopLeading)
                                    .clipped()
                                    .test_id("svg-159"))
                                .test_id("div-157"),
                                text("Dunes")
                                .font_size(13.0)
                                .foreground_color(Color::rgb(169, 169, 177))
                                .line_height(20.15)
                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                .test_id("figcaption-160"),
                            )
                            .spacing(8.0)
                            .alignment(HorizontalAlignment::Leading)
                            .frame_aligned(125.5, 120.1484, Alignment::TopLeading)
                            .test_id("figure-156")
                            .padding_edge(Edge::Leading, 10.0),
                            vstack!(
                                spacer()
                                .frame_height_aligned(92.0, Alignment::TopLeading)
                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                .background_gradient(Gradient::linear(Color::rgb(236, 243, 245), Color::rgb(211, 229, 235)).direction(UnitPoint { x: 0.5, y: 0.0 }, UnitPoint { x: 0.5, y: 1.0 }))
                                .corner_radius(12.0)
                                .clipped()
                                .overlay(UnitPoint::TOP_LEADING, spacer()
                                    .frame_aligned(14.0, 14.0, Alignment::TopLeading)
                                    .background_color(Color::rgb(255, 255, 255))
                                    .corner_radius(7.0)
                                    .test_id("span-163")
                                    .offset(80.3125, 16.0))
                                .overlay(UnitPoint::TOP_LEADING, spacer()
                                    .frame_aligned(125.5, 92.0, Alignment::TopLeading)
                                    .clipped()
                                    .test_id("svg-164"))
                                .test_id("div-162"),
                                text("Tide")
                                .font_size(13.0)
                                .foreground_color(Color::rgb(169, 169, 177))
                                .line_height(20.15)
                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                .test_id("figcaption-165"),
                            )
                            .spacing(8.0)
                            .alignment(HorizontalAlignment::Leading)
                            .frame_aligned(125.5, 120.1484, Alignment::TopLeading)
                            .test_id("figure-161")
                            .padding_edge(Edge::Leading, 10.0),
                            vstack!(
                                spacer()
                                .frame_height_aligned(92.0, Alignment::TopLeading)
                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                .background_gradient(Gradient::linear(Color::rgb(250, 247, 242), Color::rgb(241, 236, 227)).direction(UnitPoint { x: 0.5, y: 0.0 }, UnitPoint { x: 0.5, y: 1.0 }))
                                .corner_radius(12.0)
                                .clipped()
                                .overlay(UnitPoint::TOP_LEADING, spacer()
                                    .frame_aligned(125.5, 92.0, Alignment::TopLeading)
                                    .clipped()
                                    .test_id("svg-168"))
                                .test_id("div-167"),
                                text("Ink")
                                .font_size(13.0)
                                .foreground_color(Color::rgb(169, 169, 177))
                                .line_height(20.15)
                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                .test_id("figcaption-169"),
                            )
                            .spacing(8.0)
                            .alignment(HorizontalAlignment::Leading)
                            .frame_aligned(125.5, 120.1484, Alignment::TopLeading)
                            .test_id("figure-166")
                            .padding_edge(Edge::Leading, 10.0),
                        )
                        .alignment(VerticalAlignment::Top)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("div-150"),
                        vstack!(
                            text("A start page with a view")
                            .font_size(21.0)
                            .font_weight(Weight::Semibold)
                            .foreground_color(Color::rgb(244, 242, 236))
                            .tracking(-0.21)
                            .line_height(32.55)
                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                            .test_id("h3-171"),
                            text("Four scenes — Hills, Dunes, Tide and Ink — each with a day and a night. The one behind this page is Hills, after dark.")
                            .foreground_color(Color::rgb(169, 169, 177))
                            .line_height(24.8)
                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                            .test_id("p-172"),
                        )
                        .spacing(8.0)
                        .alignment(HorizontalAlignment::Leading)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("div-170"),
                    )
                    .spacing(22.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_length(29.0)
                    .frame_aligned(590.0, 300.1406, Alignment::TopLeading)
                    .background_color(Color::rgba(255, 255, 255, 9))
                    .corner_radius(24.0)
                    .border(Color::rgba(255, 255, 255, 20), 1.0)
                    .test_id("article-149"),
                    vstack!(
                        hstack!(
                            vstack!(
                                spacer()
                                .frame_aligned(85.5938, 8.0, Alignment::TopLeading)
                                .background_color(Color::rgb(43, 42, 40))
                                .corner_radius(3.0)
                                .test_id("span-193"),
                                spacer()
                                .frame_aligned(128.3906, 5.0, Alignment::TopLeading)
                                .background_color(Color::rgb(213, 209, 202))
                                .corner_radius(2.5)
                                .test_id("span-194"),
                                spacer()
                                .frame_aligned(105.5703, 5.0, Alignment::TopLeading)
                                .background_color(Color::rgb(213, 209, 202))
                                .corner_radius(2.5)
                                .test_id("span-195"),
                            )
                            .spacing(7.0)
                            .alignment(HorizontalAlignment::Leading)
                            .padding_length(14.0)
                            .frame_aligned(170.6641, 128.0, Alignment::TopLeading)
                            .background_color(Color::rgb(246, 244, 240))
                            .corner_radius(14.0)
                            .test_id("div-192"),
                            vstack!(
                                text("This page crashed")
                                .font_size(12.0)
                                .font_weight(Weight::Semibold)
                                .foreground_color(Color::rgb(244, 242, 236))
                                .line_height(18.6)
                                .multiline_text_alignment(TextAlignment::Center)
                                .test_id("span-197"),
                                text("Reload")
                                .font_size(11.0)
                                .font_weight(Weight::Semibold)
                                .foreground_color(Color::rgb(16, 16, 14))
                                .line_height(17.05)
                                .multiline_text_alignment(TextAlignment::Center)
                                .padding_edge(Edge::Top, 3.0)
                                .padding_edge(Edge::Trailing, 10.0)
                                .padding_edge(Edge::Bottom, 3.0)
                                .padding_edge(Edge::Leading, 10.0)
                                .background_color(Color::rgb(240, 106, 76))
                                .corner_radius(11.5234)
                                .test_id("span-198"),
                            )
                            .spacing(8.0)
                            .alignment(HorizontalAlignment::Center)
                            .padding_length(13.5)
                            .frame_aligned(170.6641, 128.0, Alignment::Center)
                            .background_color(Color::rgba(240, 106, 76, 18))
                            .corner_radius(14.0)
                            .border(Color::rgba(240, 106, 76, 153), 1.5)
                            .border_style(BorderStyle::Dashed)
                            .test_id("div-196")
                            .padding_edge(Edge::Leading, 10.0),
                            vstack!(
                                spacer()
                                .frame_aligned(74.1875, 8.0, Alignment::TopLeading)
                                .background_color(Color::rgb(43, 42, 40))
                                .corner_radius(3.0)
                                .test_id("span-200"),
                                spacer()
                                .frame_aligned(122.6953, 5.0, Alignment::TopLeading)
                                .background_color(Color::rgb(213, 209, 202))
                                .corner_radius(2.5)
                                .test_id("span-201"),
                                spacer()
                                .frame_aligned(99.8672, 5.0, Alignment::TopLeading)
                                .background_color(Color::rgb(213, 209, 202))
                                .corner_radius(2.5)
                                .test_id("span-202"),
                            )
                            .spacing(7.0)
                            .alignment(HorizontalAlignment::Leading)
                            .padding_length(14.0)
                            .frame_aligned(170.6719, 128.0, Alignment::TopLeading)
                            .background_color(Color::rgb(246, 244, 240))
                            .corner_radius(14.0)
                            .test_id("div-199")
                            .padding_edge(Edge::Leading, 10.0),
                        )
                        .alignment(VerticalAlignment::Top)
                        .frame_height_aligned(128.0, Alignment::TopLeading)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("div-191"),
                        vstack!(
                            text("Pages keep to themselves")
                            .font_size(21.0)
                            .font_weight(Weight::Semibold)
                            .foreground_color(Color::rgb(244, 242, 236))
                            .tracking(-0.21)
                            .line_height(32.55)
                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                            .test_id("h3-204"),
                            text("When a page fails, only that page goes — your other tabs carry on. Runaway scripts meet a watchdog, and every page has a memory ceiling.")
                            .foreground_color(Color::rgb(169, 169, 177))
                            .line_height(24.8)
                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                            .test_id("p-205"),
                        )
                        .spacing(8.0)
                        .alignment(HorizontalAlignment::Leading)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("div-203"),
                    )
                    .spacing(22.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_length(29.0)
                    .frame_aligned(590.0, 322.9375, Alignment::TopLeading)
                    .background_color(Color::rgba(255, 255, 255, 9))
                    .corner_radius(24.0)
                    .border(Color::rgba(255, 255, 255, 20), 1.0)
                    .test_id("article-190")
                    .padding_edge(Edge::Top, 20.0),
                )
                .alignment(HorizontalAlignment::Leading),
                vstack!(
                    vstack!(
                        vstack!(
                            hstack!(
                                spacer()
                                .frame_aligned(9.0, 9.0, Alignment::TopLeading)
                                .background_color(Color::rgb(255, 95, 87))
                                .corner_radius(4.5)
                                .test_id("span-176"),
                                spacer()
                                .frame_aligned(9.0, 9.0, Alignment::TopLeading)
                                .background_color(Color::rgb(254, 188, 46))
                                .corner_radius(4.5)
                                .test_id("span-177"),
                                spacer()
                                .frame_aligned(9.0, 9.0, Alignment::TopLeading)
                                .background_color(Color::rgb(40, 200, 64))
                                .corner_radius(4.5)
                                .test_id("span-178"),
                                spacer()
                                .frame_aligned(34.0, 24.0, Alignment::TopLeading)
                                .background_color(Color::rgb(46, 46, 49))
                                .corner_radius(7.0)
                                .test_id("span-179")
                                .padding_edge(Edge::Leading, 6.0),
                                hstack!(
                                    text("Search or enter website name")
                                    .font_size(12.0)
                                    .foreground_color(Color::rgb(154, 154, 161))
                                    .line_height(18.6),
                                )
                                .alignment(VerticalAlignment::Center)
                                .padding_edge(Edge::Trailing, 10.0)
                                .padding_edge(Edge::Leading, 10.0)
                                .frame_height_aligned(24.0, Alignment::Leading)
                                .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading)
                                .background_color(Color::rgb(46, 46, 49))
                                .corner_radius(7.0)
                                .test_id("span-180"),
                                spacer()
                                .frame_aligned(26.0, 24.0, Alignment::TopLeading)
                                .background_color(Color::rgb(46, 46, 49))
                                .corner_radius(7.0)
                                .test_id("span-181"),
                                spacer()
                                .frame_aligned(26.0, 24.0, Alignment::TopLeading)
                                .background_color(Color::rgb(46, 46, 49))
                                .corner_radius(7.0)
                                .test_id("span-182"),
                            )
                            .spacing(8.0)
                            .alignment(VerticalAlignment::Center)
                            .padding_edge(Edge::Trailing, 12.0)
                            .padding_edge(Edge::Bottom, 1.0)
                            .padding_edge(Edge::Leading, 12.0)
                            .frame_height_aligned(45.0, Alignment::Leading)
                            .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading)
                            .background_color(Color::rgb(30, 30, 32))
                            .overlay(UnitPoint::BOTTOM, spacer().frame_height(1.0).background_color(Color::rgba(255, 255, 255, 18)))
                            .test_id("div-175"),
                            vstack!(
                                spacer()
                                .frame_aligned(199.1953, 8.0, Alignment::TopLeading)
                                .background_color(Color::rgba(255, 255, 255, 41))
                                .corner_radius(3.0)
                                .test_id("span-184"),
                                spacer()
                                .frame_aligned(378.4766, 5.0, Alignment::TopLeading)
                                .background_color(Color::rgba(255, 255, 255, 20))
                                .corner_radius(2.5)
                                .test_id("span-185"),
                                spacer()
                                .frame_aligned(328.6797, 5.0, Alignment::TopLeading)
                                .background_color(Color::rgba(255, 255, 255, 20))
                                .corner_radius(2.5)
                                .test_id("span-186"),
                            )
                            .spacing(7.0)
                            .alignment(HorizontalAlignment::Leading)
                            .padding_edge(Edge::Top, 14.0)
                            .padding_edge(Edge::Trailing, 16.0)
                            .padding_edge(Edge::Bottom, 14.0)
                            .padding_edge(Edge::Leading, 16.0)
                            .frame_max(f64::INFINITY, f64::INFINITY, Alignment::TopLeading)
                            .test_id("div-183"),
                        )
                        .alignment(HorizontalAlignment::Leading)
                        .padding_length(1.0)
                        .frame_height_aligned(130.0, Alignment::TopLeading)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .background_color(Color::rgb(19, 20, 25))
                        .corner_radius(14.0)
                        .border(Color::rgba(255, 255, 255, 15), 1.0)
                        .clipped()
                        .test_id("div-174"),
                        vstack!(
                            text("One slim bar")
                            .font_size(21.0)
                            .font_weight(Weight::Semibold)
                            .foreground_color(Color::rgb(244, 242, 236))
                            .tracking(-0.21)
                            .line_height(32.55)
                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                            .test_id("h3-188"),
                            text("Address, tabs and controls share a single compact bar. Tab previews, popovers and find appear when you ask, and leave when you’re done.")
                            .foreground_color(Color::rgb(169, 169, 177))
                            .line_height(24.8)
                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                            .test_id("p-189"),
                        )
                        .spacing(8.0)
                        .alignment(HorizontalAlignment::Leading)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("div-187"),
                    )
                    .spacing(22.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_length(29.0)
                    .frame_aligned(590.0, 300.1406, Alignment::TopLeading)
                    .background_color(Color::rgba(255, 255, 255, 9))
                    .corner_radius(24.0)
                    .border(Color::rgba(255, 255, 255, 20), 1.0)
                    .test_id("article-173"),
                    vstack!(
                        vstack!(
                            hstack!(
                                text("Elements")
                                .font_size(12.0)
                                .font_weight(Weight::Semibold)
                                .foreground_color(Color::rgb(244, 242, 236))
                                .line_height(18.6)
                                .frame_height_aligned(18.5938, Alignment::TopLeading)
                                .test_id("span-209"),
                                text("Console")
                                .font_size(12.0)
                                .foreground_color(Color::rgb(154, 154, 161))
                                .line_height(18.6)
                                .frame_height_aligned(18.5938, Alignment::TopLeading)
                                .test_id("span-210"),
                                text("Sources")
                                .font_size(12.0)
                                .foreground_color(Color::rgb(154, 154, 161))
                                .line_height(18.6)
                                .frame_height_aligned(18.5938, Alignment::TopLeading)
                                .test_id("span-211"),
                                text("Network")
                                .font_size(12.0)
                                .foreground_color(Color::rgb(154, 154, 161))
                                .line_height(18.6)
                                .frame_height_aligned(18.5938, Alignment::TopLeading)
                                .test_id("span-212"),
                                text("Performance")
                                .font_size(12.0)
                                .foreground_color(Color::rgb(154, 154, 161))
                                .line_height(18.6)
                                .frame_height_aligned(18.5938, Alignment::TopLeading)
                                .test_id("span-213"),
                                text("Memory")
                                .font_size(12.0)
                                .foreground_color(Color::rgb(154, 154, 161))
                                .line_height(18.6)
                                .frame_height_aligned(18.5938, Alignment::TopLeading)
                                .test_id("span-214"),
                                text("Application")
                                .font_size(12.0)
                                .foreground_color(Color::rgb(154, 154, 161))
                                .line_height(18.6)
                                .frame_height_aligned(18.5938, Alignment::TopLeading)
                                .test_id("span-215"),
                                text("Recorder")
                                .font_size(12.0)
                                .foreground_color(Color::rgb(240, 106, 76))
                                .line_height(18.6)
                                .frame_height_aligned(18.5938, Alignment::TopLeading)
                                .test_id("span-216"),
                            )
                            .spacing(14.0)
                            .alignment(VerticalAlignment::Top)
                            .wrapping()
                            .line_spacing(4.0)
                            .padding_edge(Edge::Top, 10.0)
                            .padding_edge(Edge::Trailing, 14.0)
                            .padding_edge(Edge::Bottom, 11.0)
                            .padding_edge(Edge::Leading, 14.0)
                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                            .overlay(UnitPoint::BOTTOM, spacer().frame_height(1.0).background_color(Color::rgba(255, 255, 255, 18)))
                            .test_id("div-208"),
                            vstack!(
                                rich_text([
                                    run("<main").foreground_color(Color::rgb(240, 106, 76)),
                                    run(" "),
                                    run("class").foreground_color(Color::rgb(233, 228, 212)),
                                    run("="),
                                    run("\"cards\"").foreground_color(Color::rgb(169, 212, 155)),
                                    run(">").foreground_color(Color::rgb(240, 106, 76)),
                                ])
                                .font_size(12.0)
                                .foreground_color(Color::rgb(154, 154, 161))
                                .line_height(20.4)
                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                .test_id("div-218"),
                                rich_text([
                                    run("<h1>").foreground_color(Color::rgb(240, 106, 76)),
                                    run("The demo page").foreground_color(Color::rgb(236, 236, 239)),
                                    run("</h1>").foreground_color(Color::rgb(240, 106, 76)),
                                ])
                                .font_size(12.0)
                                .foreground_color(Color::rgb(154, 154, 161))
                                .line_height(20.4)
                                .padding_edge(Edge::Leading, 16.0)
                                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                                .test_id("div-223"),
                            )
                            .alignment(HorizontalAlignment::Leading)
                            .padding_edge(Edge::Top, 10.0)
                            .padding_edge(Edge::Trailing, 14.0)
                            .padding_edge(Edge::Bottom, 10.0)
                            .padding_edge(Edge::Leading, 14.0)
                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                            .test_id("div-217"),
                        )
                        .alignment(HorizontalAlignment::Leading)
                        .padding_length(1.0)
                        .frame_height_aligned(130.0, Alignment::TopLeading)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .background_color(Color::rgb(19, 20, 25))
                        .corner_radius(14.0)
                        .border(Color::rgba(255, 255, 255, 15), 1.0)
                        .clipped()
                        .test_id("div-207"),
                        vstack!(
                            text("DevTools, built in")
                            .font_size(21.0)
                            .font_weight(Weight::Semibold)
                            .foreground_color(Color::rgb(244, 242, 236))
                            .tracking(-0.21)
                            .line_height(32.55)
                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                            .test_id("h3-228"),
                            text("Elements, Console, Sources, Network, Performance, Memory, Application — and a Recorder to replay what you did.")
                            .foreground_color(Color::rgb(169, 169, 177))
                            .line_height(24.8)
                            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                            .test_id("p-229"),
                        )
                        .spacing(8.0)
                        .alignment(HorizontalAlignment::Leading)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("div-227"),
                    )
                    .spacing(22.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_length(29.0)
                    .frame_aligned(590.0, 322.9375, Alignment::TopLeading)
                    .background_color(Color::rgba(255, 255, 255, 9))
                    .corner_radius(24.0)
                    .border(Color::rgba(255, 255, 255, 20), 1.0)
                    .test_id("article-206")
                    .padding_edge(Edge::Top, 20.0),
                )
                .alignment(HorizontalAlignment::Leading)
                .padding_edge(Edge::Leading, 20.0),
            )
            .alignment(VerticalAlignment::Top)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
            .test_id("div-148")
            .padding_edge(Edge::Top, 48.0),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding_edge(Edge::Top, 120.0)
        .padding_edge(Edge::Trailing, 32.0)
        .padding_edge(Edge::Leading, 32.0)
        .frame_max(1264.0, f64::MAX, Alignment::TopLeading)
        .test_id("section-143")
        .padding_edge(Edge::Leading, 88.0)
        .padding_edge(Edge::Trailing, 88.0),
        vstack!(
            hstack!(
                vstack!(
                    text("UNDER THE HOOD")
                    .font_size(13.0)
                    .foreground_color(Color::rgb(240, 106, 76))
                    .tracking(1.04)
                    .line_height(20.15)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("p-233"),
                    vstack!(
                        text("From the first byte")
                        .font_size(56.0)
                        .font_weight(Weight::Semibold)
                        .foreground_color(Color::rgb(244, 242, 236))
                        .tracking(-1.96)
                        .line_height(58.24)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading),
                        spacer().frame(0.0, 68.0)
                        .test_id("br-235")
                        .padding_edge(Edge::Leading, 421.9922),
                        text("to the last pixel.")
                        .font_size(56.0)
                        .font_weight(Weight::Semibold)
                        .foreground_color(Color::rgb(244, 242, 236))
                        .tracking(-1.96)
                        .line_height(58.24)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading),
                    )
                    .alignment(HorizontalAlignment::Leading)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("h2-234"),
                )
                .spacing(16.0)
                .alignment(HorizontalAlignment::Leading)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                .test_id("div-232"),
                text("No Chromium inside, no WebKit underneath. hop runs on an engine of its own, written from scratch in Rust — and every tab, menu and popover is drawn by bunny_ui, our own interface toolkit. It’s why the whole browser fits in the page above.")
                .font_size(19.0)
                .foreground_color(Color::rgb(169, 169, 177))
                .line_height(29.45)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                .test_id("p-236"),
            )
            .spacing(64.0)
            .alignment(VerticalAlignment::Bottom)
            .wrapping()
            .line_spacing(32.0)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::BottomLeading)
            .test_id("div-231"),
            hstack!(
                vstack!(
                    spacer()
                    .frame_aligned(28.0, 28.0, Alignment::TopLeading)
                    .clipped()
                    .test_id("svg-239"),
                    text("Rust, end to end")
                    .font_size(20.0)
                    .font_weight(Weight::Semibold)
                    .foreground_color(Color::rgb(244, 242, 236))
                    .line_height(31.0)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("h3-240"),
                    text("Parsing, style, layout, script, networking and paint — one language, one codebase.")
                    .foreground_color(Color::rgb(180, 180, 187))
                    .line_height(24.8)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("p-241"),
                )
                .spacing(14.0)
                .alignment(HorizontalAlignment::Leading)
                .padding_length(29.0)
                .frame_aligned(386.6641, 196.5938, Alignment::TopLeading)
                .background_gradient(Gradient::linear(Color::rgba(240, 106, 76, 31), Color::rgba(240, 106, 76, 8)).direction(UnitPoint { x: 0.5, y: 0.0 }, UnitPoint { x: 0.5, y: 1.0 }))
                .corner_radius(24.0)
                .border(Color::rgba(240, 106, 76, 64), 1.0)
                .test_id("div-238"),
                vstack!(
                    image(picture_hop_icon_light_2())
                    .resizable()
                    .frame_aligned(30.0, 30.0, Alignment::TopLeading)
                    .clipped()
                    .test_id("img-243"),
                    text("Drawn by bunny_ui")
                    .font_size(20.0)
                    .font_weight(Weight::Semibold)
                    .foreground_color(Color::rgb(244, 242, 236))
                    .line_height(31.0)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("h3-244"),
                    text("Our own toolkit renders the interface — the same on your desktop as in WebAssembly.")
                    .foreground_color(Color::rgb(169, 169, 177))
                    .line_height(24.8)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("p-245"),
                )
                .spacing(14.0)
                .alignment(HorizontalAlignment::Leading)
                .padding_length(29.0)
                .frame_aligned(386.6641, 196.5938, Alignment::TopLeading)
                .background_color(Color::rgba(255, 255, 255, 9))
                .corner_radius(24.0)
                .border(Color::rgba(255, 255, 255, 20), 1.0)
                .test_id("div-242")
                .padding_edge(Edge::Leading, 20.0),
                vstack!(
                    spacer()
                    .frame_aligned(28.0, 28.0, Alignment::TopLeading)
                    .clipped()
                    .test_id("svg-247"),
                    text("Nothing underneath")
                    .font_size(20.0)
                    .font_weight(Weight::Semibold)
                    .foreground_color(Color::rgb(244, 242, 236))
                    .line_height(31.0)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("h3-248"),
                    text("No borrowed engine, no system web view. hop lays out and paints every page itself.")
                    .foreground_color(Color::rgb(169, 169, 177))
                    .line_height(24.8)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("p-249"),
                )
                .spacing(14.0)
                .alignment(HorizontalAlignment::Leading)
                .padding_length(29.0)
                .frame_aligned(386.6641, 196.5938, Alignment::TopLeading)
                .background_color(Color::rgba(255, 255, 255, 9))
                .corner_radius(24.0)
                .border(Color::rgba(255, 255, 255, 20), 1.0)
                .test_id("div-246")
                .padding_edge(Edge::Leading, 20.0),
            )
            .alignment(VerticalAlignment::Top)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
            .test_id("div-237")
            .padding_edge(Edge::Top, 52.0),
            hstack!(
                vstack!(
                    vstack!(
                        text("HTML")
                        .font_size(13.0)
                        .foreground_color(Color::rgb(240, 106, 76))
                        .tracking(0.52)
                        .line_height(20.15)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-252"),
                        text("The WHATWG tokenizer and tree construction, with script-aware parsing.")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(180, 180, 187))
                        .line_height(22.5)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-253"),
                    )
                    .spacing(8.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_edge(Edge::Top, 24.0)
                    .padding_edge(Edge::Trailing, 24.0)
                    .padding_edge(Edge::Bottom, 26.0)
                    .padding_edge(Edge::Leading, 24.0)
                    .frame_aligned(298.75, 145.6484, Alignment::TopLeading)
                    .background_color(Color::rgb(11, 15, 23))
                    .test_id("div-251"),
                    vstack!(
                        text("WebAssembly")
                        .font_size(13.0)
                        .foreground_color(Color::rgb(240, 106, 76))
                        .tracking(0.52)
                        .line_height(20.15)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-264"),
                        text("An interpreter and a JIT, both written for hop.")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(180, 180, 187))
                        .line_height(22.5)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-265"),
                    )
                    .spacing(8.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_edge(Edge::Top, 24.0)
                    .padding_edge(Edge::Trailing, 24.0)
                    .padding_edge(Edge::Bottom, 26.0)
                    .padding_edge(Edge::Leading, 24.0)
                    .frame_aligned(298.75, 145.6484, Alignment::TopLeading)
                    .background_color(Color::rgb(11, 15, 23))
                    .test_id("div-263")
                    .padding_edge(Edge::Top, 1.0),
                    vstack!(
                        text("Images")
                        .font_size(13.0)
                        .foreground_color(Color::rgb(240, 106, 76))
                        .tracking(0.52)
                        .line_height(20.15)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-276"),
                        text("PNG, APNG, GIF, JPEG, WebP, BMP and ICO, animation included.")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(180, 180, 187))
                        .line_height(22.5)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-277"),
                    )
                    .spacing(8.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_edge(Edge::Top, 24.0)
                    .padding_edge(Edge::Trailing, 24.0)
                    .padding_edge(Edge::Bottom, 26.0)
                    .padding_edge(Edge::Leading, 24.0)
                    .frame_aligned(298.75, 123.1484, Alignment::TopLeading)
                    .background_color(Color::rgb(11, 15, 23))
                    .test_id("div-275")
                    .padding_edge(Edge::Top, 1.0),
                )
                .alignment(HorizontalAlignment::Leading),
                vstack!(
                    vstack!(
                        text("CSS")
                        .font_size(13.0)
                        .foreground_color(Color::rgb(240, 106, 76))
                        .tracking(0.52)
                        .line_height(20.15)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-255"),
                        text("Selectors, cascade, inheritance and computed style.")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(180, 180, 187))
                        .line_height(22.5)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-256"),
                    )
                    .spacing(8.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_edge(Edge::Top, 24.0)
                    .padding_edge(Edge::Trailing, 24.0)
                    .padding_edge(Edge::Bottom, 26.0)
                    .padding_edge(Edge::Leading, 24.0)
                    .frame_aligned(298.75, 145.6484, Alignment::TopLeading)
                    .background_color(Color::rgb(11, 15, 23))
                    .test_id("div-254"),
                    vstack!(
                        text("Networking")
                        .font_size(13.0)
                        .foreground_color(Color::rgb(240, 106, 76))
                        .tracking(0.52)
                        .line_height(20.15)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-267"),
                        text("Fetch, CORS, HTTP/1.1 and HTTP/2, WebSocket, cookies and cache.")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(180, 180, 187))
                        .line_height(22.5)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-268"),
                    )
                    .spacing(8.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_edge(Edge::Top, 24.0)
                    .padding_edge(Edge::Trailing, 24.0)
                    .padding_edge(Edge::Bottom, 26.0)
                    .padding_edge(Edge::Leading, 24.0)
                    .frame_aligned(298.75, 145.6484, Alignment::TopLeading)
                    .background_color(Color::rgb(11, 15, 23))
                    .test_id("div-266")
                    .padding_edge(Edge::Top, 1.0),
                    vstack!(
                        text("Media")
                        .font_size(13.0)
                        .foreground_color(Color::rgb(240, 106, 76))
                        .tracking(0.52)
                        .line_height(20.15)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-279"),
                        text("AV1, H.264 and VP9 video, audio and HLS streams.")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(180, 180, 187))
                        .line_height(22.5)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-280"),
                    )
                    .spacing(8.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_edge(Edge::Top, 24.0)
                    .padding_edge(Edge::Trailing, 24.0)
                    .padding_edge(Edge::Bottom, 26.0)
                    .padding_edge(Edge::Leading, 24.0)
                    .frame_aligned(298.75, 123.1484, Alignment::TopLeading)
                    .background_color(Color::rgb(11, 15, 23))
                    .test_id("div-278")
                    .padding_edge(Edge::Top, 1.0),
                )
                .alignment(HorizontalAlignment::Leading)
                .padding_edge(Edge::Leading, 1.0),
                vstack!(
                    vstack!(
                        text("Layout")
                        .font_size(13.0)
                        .foreground_color(Color::rgb(240, 106, 76))
                        .tracking(0.52)
                        .line_height(20.15)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-258"),
                        text("Block, inline, flex, grid, tables, floats and positioned boxes.")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(180, 180, 187))
                        .line_height(22.5)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-259"),
                    )
                    .spacing(8.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_edge(Edge::Top, 24.0)
                    .padding_edge(Edge::Trailing, 24.0)
                    .padding_edge(Edge::Bottom, 26.0)
                    .padding_edge(Edge::Leading, 24.0)
                    .frame_aligned(298.75, 145.6484, Alignment::TopLeading)
                    .background_color(Color::rgb(11, 15, 23))
                    .test_id("div-257"),
                    vstack!(
                        text("Rendering")
                        .font_size(13.0)
                        .foreground_color(Color::rgb(240, 106, 76))
                        .tracking(0.52)
                        .line_height(20.15)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-270"),
                        text("Vector paths, anti-aliasing and compositing, rasterized in software.")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(180, 180, 187))
                        .line_height(22.5)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-271"),
                    )
                    .spacing(8.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_edge(Edge::Top, 24.0)
                    .padding_edge(Edge::Trailing, 24.0)
                    .padding_edge(Edge::Bottom, 26.0)
                    .padding_edge(Edge::Leading, 24.0)
                    .frame_aligned(298.75, 145.6484, Alignment::TopLeading)
                    .background_color(Color::rgb(11, 15, 23))
                    .test_id("div-269")
                    .padding_edge(Edge::Top, 1.0),
                    vstack!(
                        text("SVG & Canvas")
                        .font_size(13.0)
                        .foreground_color(Color::rgb(240, 106, 76))
                        .tracking(0.52)
                        .line_height(20.15)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-282"),
                        text("SVG and the Canvas 2D context on hop’s own vector engine.")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(180, 180, 187))
                        .line_height(22.5)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-283"),
                    )
                    .spacing(8.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_edge(Edge::Top, 24.0)
                    .padding_edge(Edge::Trailing, 24.0)
                    .padding_edge(Edge::Bottom, 26.0)
                    .padding_edge(Edge::Leading, 24.0)
                    .frame_aligned(298.75, 123.1484, Alignment::TopLeading)
                    .background_color(Color::rgb(11, 15, 23))
                    .test_id("div-281")
                    .padding_edge(Edge::Top, 1.0),
                )
                .alignment(HorizontalAlignment::Leading)
                .padding_edge(Edge::Leading, 1.0),
                vstack!(
                    vstack!(
                        text("JavaScript")
                        .font_size(13.0)
                        .foreground_color(Color::rgb(240, 106, 76))
                        .tracking(0.52)
                        .line_height(20.15)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-261"),
                        text("Its own parser, bytecode compiler, VM and garbage-collected heap.")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(180, 180, 187))
                        .line_height(22.5)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-262"),
                    )
                    .spacing(8.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_edge(Edge::Top, 24.0)
                    .padding_edge(Edge::Trailing, 24.0)
                    .padding_edge(Edge::Bottom, 26.0)
                    .padding_edge(Edge::Leading, 24.0)
                    .frame_aligned(298.75, 145.6484, Alignment::TopLeading)
                    .background_color(Color::rgb(11, 15, 23))
                    .test_id("div-260"),
                    vstack!(
                        text("Text & fonts")
                        .font_size(13.0)
                        .foreground_color(Color::rgb(240, 106, 76))
                        .tracking(0.52)
                        .line_height(20.15)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-273"),
                        text("Line breaking, bidi, WOFF2, CFF2 and variable fonts.")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(180, 180, 187))
                        .line_height(22.5)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-274"),
                    )
                    .spacing(8.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_edge(Edge::Top, 24.0)
                    .padding_edge(Edge::Trailing, 24.0)
                    .padding_edge(Edge::Bottom, 26.0)
                    .padding_edge(Edge::Leading, 24.0)
                    .frame_aligned(298.75, 145.6484, Alignment::TopLeading)
                    .background_color(Color::rgb(11, 15, 23))
                    .test_id("div-272")
                    .padding_edge(Edge::Top, 1.0),
                    vstack!(
                        text("Storage")
                        .font_size(13.0)
                        .foreground_color(Color::rgb(240, 106, 76))
                        .tracking(0.52)
                        .line_height(20.15)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-285"),
                        text("Web Storage and IndexedDB.")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(180, 180, 187))
                        .line_height(22.5)
                        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                        .test_id("span-286"),
                    )
                    .spacing(8.0)
                    .alignment(HorizontalAlignment::Leading)
                    .padding_edge(Edge::Top, 24.0)
                    .padding_edge(Edge::Trailing, 24.0)
                    .padding_edge(Edge::Bottom, 26.0)
                    .padding_edge(Edge::Leading, 24.0)
                    .frame_aligned(298.75, 123.1484, Alignment::TopLeading)
                    .background_color(Color::rgb(11, 15, 23))
                    .test_id("div-284")
                    .padding_edge(Edge::Top, 1.0),
                )
                .alignment(HorizontalAlignment::Leading)
                .padding_edge(Edge::Leading, 1.0),
            )
            .alignment(VerticalAlignment::Top)
            .padding_length(1.0)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
            .background_color(Color::rgba(255, 255, 255, 20))
            .corner_radius(24.0)
            .border(Color::rgba(255, 255, 255, 20), 1.0)
            .clipped()
            .test_id("div-250")
            .padding_edge(Edge::Top, 20.0),
            text("Measured against test262 and the web-platform-tests.")
            .font_size(15.0)
            .foreground_color(Color::rgb(142, 142, 150))
            .line_height(23.25)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
            .test_id("p-287")
            .padding_edge(Edge::Top, 20.0),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding_edge(Edge::Top, 140.0)
        .padding_edge(Edge::Trailing, 32.0)
        .padding_edge(Edge::Leading, 32.0)
        .frame_max(1264.0, f64::MAX, Alignment::TopLeading)
        .test_id("section-230")
        .padding_edge(Edge::Leading, 88.0)
        .padding_edge(Edge::Trailing, 88.0),
        vstack!(
            vstack!(
                image(picture_hop_icon_light_2())
                .resizable()
                .frame_aligned(64.0, 64.0, Alignment::Top)
                .clipped()
                .test_id("img-290"),
                text("Free, on every desk.")
                .font_size(56.0)
                .font_weight(Weight::Semibold)
                .foreground_color(Color::rgb(244, 242, 236))
                .tracking(-1.96)
                .line_height(58.24)
                .multiline_text_alignment(TextAlignment::Center)
                .test_id("h2-291"),
                text("hop costs nothing on macOS, Windows and Linux. Liked the one above? It’s the same browser.")
                .font_size(19.0)
                .foreground_color(Color::rgb(169, 169, 177))
                .line_height(29.45)
                .multiline_text_alignment(TextAlignment::Center)
                .test_id("p-292"),
            )
            .spacing(16.0)
            .alignment(HorizontalAlignment::Center)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::Top)
            .test_id("div-289"),
            hstack!(
                vstack!(
                    hstack!(
                        hstack!(
                            spacer()
                            .frame_aligned(22.0, 22.0, Alignment::TopLeading)
                            .clipped()
                            .test_id("svg-297"),
                        )
                        .alignment(VerticalAlignment::Center)
                        .frame_aligned(44.0, 44.0, Alignment::Center)
                        .background_color(Color::rgba(255, 255, 255, 15))
                        .corner_radius(12.0)
                        .test_id("span-296"),
                        vstack!(
                            text("macOS")
                            .font_size(18.0)
                            .font_weight(Weight::Semibold)
                            .foreground_color(Color::rgb(244, 242, 236))
                            .line_height(27.9)
                            .frame_width_aligned(83.2734, Alignment::TopLeading)
                            .test_id("span-299"),
                            text("Version 0.1.0")
                            .font_size(14.0)
                            .foreground_color(Color::rgb(154, 154, 161))
                            .line_height(21.7)
                            .frame_width_aligned(83.2734, Alignment::TopLeading)
                            .test_id("span-300"),
                        )
                        .alignment(HorizontalAlignment::Leading)
                        .test_id("div-298"),
                    )
                    .spacing(12.0)
                    .alignment(VerticalAlignment::Center)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading)
                    .test_id("div-295"),
                    hstack!(
                        spacer()
                        .frame_aligned(16.0, 16.0, Alignment::TopLeading)
                        .clipped()
                        .test_id("svg-302")
                        .on_click(|| {}),
                        text("Download for macOS")
                        .font_size(15.0)
                        .font_weight(Weight::Semibold)
                        .foreground_color(Color::rgb(16, 16, 14))
                        .line_height(23.25),
                    )
                    .spacing(8.0)
                    .alignment(VerticalAlignment::Center)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::Center)
                    .frame_min(0.0, 46.0, Alignment::Center)
                    .background_color(Color::rgb(240, 106, 76))
                    .corner_radius(23.0)
                    .test_id("a-301")
                    .on_click(|| {}),
                )
                .spacing(18.0)
                .alignment(HorizontalAlignment::Leading)
                .padding_length(27.0)
                .frame_aligned(288.0, 169.5938, Alignment::TopLeading)
                .background_color(Color::rgba(255, 255, 255, 9))
                .corner_radius(22.0)
                .border(Color::rgba(255, 255, 255, 20), 1.0)
                .test_id("div-294"),
                vstack!(
                    hstack!(
                        hstack!(
                            spacer()
                            .frame_aligned(22.0, 22.0, Alignment::TopLeading)
                            .clipped()
                            .test_id("svg-306"),
                        )
                        .alignment(VerticalAlignment::Center)
                        .frame_aligned(44.0, 44.0, Alignment::Center)
                        .background_color(Color::rgba(255, 255, 255, 15))
                        .corner_radius(12.0)
                        .test_id("span-305"),
                        vstack!(
                            text("Windows")
                            .font_size(18.0)
                            .font_weight(Weight::Semibold)
                            .foreground_color(Color::rgb(244, 242, 236))
                            .line_height(27.9)
                            .frame_width_aligned(83.2734, Alignment::TopLeading)
                            .test_id("span-308"),
                            text("Version 0.1.0")
                            .font_size(14.0)
                            .foreground_color(Color::rgb(154, 154, 161))
                            .line_height(21.7)
                            .frame_width_aligned(83.2734, Alignment::TopLeading)
                            .test_id("span-309"),
                        )
                        .alignment(HorizontalAlignment::Leading)
                        .test_id("div-307"),
                    )
                    .spacing(12.0)
                    .alignment(VerticalAlignment::Center)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading)
                    .test_id("div-304"),
                    hstack!(
                        spacer()
                        .frame_aligned(16.0, 16.0, Alignment::TopLeading)
                        .clipped()
                        .test_id("svg-311")
                        .on_click(|| {}),
                        text("Download for Windows")
                        .font_size(15.0)
                        .font_weight(Weight::Semibold)
                        .foreground_color(Color::rgb(16, 16, 14))
                        .line_height(23.25),
                    )
                    .spacing(8.0)
                    .alignment(VerticalAlignment::Center)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::Center)
                    .frame_min(0.0, 46.0, Alignment::Center)
                    .background_color(Color::rgb(240, 106, 76))
                    .corner_radius(23.0)
                    .test_id("a-310")
                    .on_click(|| {}),
                )
                .spacing(18.0)
                .alignment(HorizontalAlignment::Leading)
                .padding_length(27.0)
                .frame_aligned(288.0, 169.5938, Alignment::TopLeading)
                .background_color(Color::rgba(255, 255, 255, 9))
                .corner_radius(22.0)
                .border(Color::rgba(255, 255, 255, 20), 1.0)
                .test_id("div-303")
                .padding_edge(Edge::Leading, 16.0),
                vstack!(
                    hstack!(
                        hstack!(
                            spacer()
                            .frame_aligned(22.0, 22.0, Alignment::TopLeading)
                            .clipped()
                            .test_id("svg-315"),
                        )
                        .alignment(VerticalAlignment::Center)
                        .frame_aligned(44.0, 44.0, Alignment::Center)
                        .background_color(Color::rgba(255, 255, 255, 15))
                        .corner_radius(12.0)
                        .test_id("span-314"),
                        vstack!(
                            text("Linux")
                            .font_size(18.0)
                            .font_weight(Weight::Semibold)
                            .foreground_color(Color::rgb(244, 242, 236))
                            .line_height(27.9)
                            .frame_width_aligned(83.2734, Alignment::TopLeading)
                            .test_id("span-317"),
                            text("Version 0.1.0")
                            .font_size(14.0)
                            .foreground_color(Color::rgb(154, 154, 161))
                            .line_height(21.7)
                            .frame_width_aligned(83.2734, Alignment::TopLeading)
                            .test_id("span-318"),
                        )
                        .alignment(HorizontalAlignment::Leading)
                        .test_id("div-316"),
                    )
                    .spacing(12.0)
                    .alignment(VerticalAlignment::Center)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading)
                    .test_id("div-313"),
                    hstack!(
                        spacer()
                        .frame_aligned(16.0, 16.0, Alignment::TopLeading)
                        .clipped()
                        .test_id("svg-320")
                        .on_click(|| {}),
                        text("Download for Linux")
                        .font_size(15.0)
                        .font_weight(Weight::Semibold)
                        .foreground_color(Color::rgb(16, 16, 14))
                        .line_height(23.25),
                    )
                    .spacing(8.0)
                    .alignment(VerticalAlignment::Center)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::Center)
                    .frame_min(0.0, 46.0, Alignment::Center)
                    .background_color(Color::rgb(240, 106, 76))
                    .corner_radius(23.0)
                    .test_id("a-319")
                    .on_click(|| {}),
                )
                .spacing(18.0)
                .alignment(HorizontalAlignment::Leading)
                .padding_length(27.0)
                .frame_aligned(288.0, 169.5938, Alignment::TopLeading)
                .background_color(Color::rgba(255, 255, 255, 9))
                .corner_radius(22.0)
                .border(Color::rgba(255, 255, 255, 20), 1.0)
                .test_id("div-312")
                .padding_edge(Edge::Leading, 16.0),
                vstack!(
                    hstack!(
                        hstack!(
                            spacer()
                            .frame_aligned(22.0, 22.0, Alignment::TopLeading)
                            .clipped()
                            .test_id("svg-324"),
                        )
                        .alignment(VerticalAlignment::Center)
                        .frame_aligned(44.0, 44.0, Alignment::Center)
                        .background_color(Color::rgba(255, 255, 255, 15))
                        .corner_radius(12.0)
                        .test_id("span-323"),
                        vstack!(
                            text("The web")
                            .font_size(18.0)
                            .font_weight(Weight::Semibold)
                            .foreground_color(Color::rgb(244, 242, 236))
                            .line_height(27.9)
                            .frame_width_aligned(111.1094, Alignment::TopLeading)
                            .test_id("span-326"),
                            text("Nothing to install")
                            .font_size(14.0)
                            .foreground_color(Color::rgb(154, 154, 161))
                            .line_height(21.7)
                            .frame_width_aligned(111.1094, Alignment::TopLeading)
                            .test_id("span-327"),
                        )
                        .alignment(HorizontalAlignment::Leading)
                        .test_id("div-325"),
                    )
                    .spacing(12.0)
                    .alignment(VerticalAlignment::Center)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading)
                    .test_id("div-322"),
                    hstack!(
                        spacer()
                        .frame_aligned(16.0, 16.0, Alignment::TopLeading)
                        .clipped()
                        .test_id("svg-329")
                        .on_click(|| {}),
                        text("Open it above")
                        .font_size(15.0)
                        .font_weight(Weight::Medium)
                        .foreground_color(Color::rgb(236, 236, 239))
                        .line_height(23.25),
                    )
                    .spacing(8.0)
                    .alignment(VerticalAlignment::Center)
                    .padding_length(1.0)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::Center)
                    .frame_min(0.0, 48.0, Alignment::Center)
                    .background_color(Color::rgba(255, 255, 255, 15))
                    .corner_radius(24.0)
                    .border(Color::rgba(255, 255, 255, 46), 1.0)
                    .test_id("a-328")
                    .on_click(|| {}),
                )
                .spacing(18.0)
                .alignment(HorizontalAlignment::Leading)
                .padding_length(27.0)
                .frame_aligned(288.0, 169.5938, Alignment::TopLeading)
                .corner_radius(22.0)
                .border(Color::rgba(255, 255, 255, 46), 1.0)
                .border_style(BorderStyle::Dashed)
                .test_id("div-321")
                .padding_edge(Edge::Leading, 16.0),
            )
            .alignment(VerticalAlignment::Top)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
            .test_id("div-293")
            .padding_edge(Edge::Top, 48.0),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding_edge(Edge::Top, 140.0)
        .padding_edge(Edge::Trailing, 32.0)
        .padding_edge(Edge::Leading, 32.0)
        .frame_max(1264.0, f64::MAX, Alignment::TopLeading)
        .test_id("section-288")
        .padding_edge(Edge::Leading, 88.0)
        .padding_edge(Edge::Trailing, 88.0),
        hstack!(
            vstack!(
                text("HOP FOR PRODUCTS")
                .font_size(13.0)
                .foreground_color(Color::rgb(240, 106, 76))
                .tracking(1.04)
                .line_height(20.15)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                .test_id("p-333"),
                text("A browser engine for the thing you’re building.")
                .font_size(46.0)
                .font_weight(Weight::Semibold)
                .foreground_color(Color::rgb(244, 242, 236))
                .tracking(-1.38)
                .line_height(48.76)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                .test_id("h2-334"),
                text("hop’s engine is a library. Put a web view inside your app, device or kiosk — and keep it on a short leash.")
                .font_size(18.0)
                .foreground_color(Color::rgb(169, 169, 177))
                .line_height(27.9)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                .test_id("p-335"),
                vstack!(
                    hstack!(
                        spacer()
                        .frame_aligned(20.0, 20.0, Alignment::TopLeading)
                        .clipped()
                        .test_id("svg-338")
                        .padding_edge(Edge::Top, 2.0),
                        rich_text([
                            run("Network allowlists").weight(Weight::Semibold).foreground_color(Color::rgb(244, 242, 236)),
                            run(" — a view reaches only the hosts you name."),
                        ])
                        .foreground_color(Color::rgb(201, 201, 207))
                        .line_height(24.8)
                        .test_id("span-339"),
                    )
                    .spacing(12.0)
                    .alignment(VerticalAlignment::Top)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("li-337"),
                    hstack!(
                        spacer()
                        .frame_aligned(20.0, 20.0, Alignment::TopLeading)
                        .clipped()
                        .test_id("svg-342")
                        .padding_edge(Edge::Top, 2.0),
                        rich_text([
                            run("Budgets").weight(Weight::Semibold).foreground_color(Color::rgb(244, 242, 236)),
                            run(" — script instructions and memory, set per view."),
                        ])
                        .foreground_color(Color::rgb(201, 201, 207))
                        .line_height(24.8)
                        .test_id("span-343"),
                    )
                    .spacing(12.0)
                    .alignment(VerticalAlignment::Top)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("li-341"),
                    hstack!(
                        spacer()
                        .frame_aligned(20.0, 20.0, Alignment::TopLeading)
                        .clipped()
                        .test_id("svg-346")
                        .padding_edge(Edge::Top, 2.0),
                        rich_text([
                            run("Crash isolation").weight(Weight::Semibold).foreground_color(Color::rgb(244, 242, 236)),
                            run(" — a failing page ends only itself; nothing unwinds into your app."),
                        ])
                        .foreground_color(Color::rgb(201, 201, 207))
                        .line_height(24.8)
                        .test_id("span-347"),
                    )
                    .spacing(12.0)
                    .alignment(VerticalAlignment::Top)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("li-345"),
                    hstack!(
                        spacer()
                        .frame_aligned(20.0, 20.0, Alignment::TopLeading)
                        .clipped()
                        .test_id("svg-350")
                        .padding_edge(Edge::Top, 2.0),
                        rich_text([
                            run("Headless").weight(Weight::Semibold).foreground_color(Color::rgb(244, 242, 236)),
                            run(" — render pages to PNG, text or layout, no window needed."),
                        ])
                        .foreground_color(Color::rgb(201, 201, 207))
                        .line_height(24.8)
                        .test_id("span-351"),
                    )
                    .spacing(12.0)
                    .alignment(VerticalAlignment::Top)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("li-349"),
                )
                .spacing(12.0)
                .alignment(HorizontalAlignment::Leading)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                .test_id("ul-336"),
                hstack!(
                    hstack!(
                        text("Talk to us about embedding")
                        .font_weight(Weight::Semibold)
                        .foreground_color(Color::rgb(16, 16, 14))
                        .line_height(24.8),
                    )
                    .alignment(VerticalAlignment::Center)
                    .padding_edge(Edge::Trailing, 24.0)
                    .padding_edge(Edge::Leading, 24.0)
                    .frame_min(0.0, 48.0, Alignment::Leading)
                    .background_color(Color::rgb(236, 236, 239))
                    .corner_radius(24.0)
                    .test_id("a-354")
                    .on_click(|| {}),
                    text("Embedding is offered through commercial partnership — partners@[YOUR-DOMAIN]")
                    .font_size(14.0)
                    .foreground_color(Color::rgb(154, 154, 161))
                    .line_height(21.7)
                    .test_id("span-355"),
                )
                .spacing(16.0)
                .alignment(VerticalAlignment::Center)
                .wrapping()
                .line_spacing(16.0)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading)
                .test_id("div-353")
                .padding_edge(Edge::Top, 8.0),
            )
            .spacing(20.0)
            .alignment(HorizontalAlignment::Leading)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
            .test_id("div-332"),
            vstack!(
                hstack!(
                    spacer()
                    .frame_aligned(9.0, 9.0, Alignment::TopLeading)
                    .background_color(Color::rgb(58, 58, 64))
                    .corner_radius(4.5)
                    .test_id("span-358"),
                    spacer()
                    .frame_aligned(9.0, 9.0, Alignment::TopLeading)
                    .background_color(Color::rgb(58, 58, 64))
                    .corner_radius(4.5)
                    .test_id("span-359"),
                    spacer()
                    .frame_aligned(9.0, 9.0, Alignment::TopLeading)
                    .background_color(Color::rgb(58, 58, 64))
                    .corner_radius(4.5)
                    .test_id("span-360"),
                    text("your_app.rs")
                    .font_size(12.0)
                    .foreground_color(Color::rgb(142, 142, 150))
                    .line_height(18.6)
                    .test_id("span-361")
                    .padding_edge(Edge::Leading, 8.0),
                )
                .spacing(8.0)
                .alignment(VerticalAlignment::Center)
                .padding_edge(Edge::Top, 12.0)
                .padding_edge(Edge::Trailing, 16.0)
                .padding_edge(Edge::Bottom, 13.0)
                .padding_edge(Edge::Leading, 16.0)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading)
                .overlay(UnitPoint::BOTTOM, spacer().frame_height(1.0).background_color(Color::rgba(255, 255, 255, 18)))
                .test_id("div-357"),
                vstack!(
                    rich_text([
                        run("let").foreground_color(Color::rgb(240, 106, 76)),
                        run(" engine = "),
                        run("Engine").foreground_color(Color::rgb(233, 228, 212)),
                        run("::builder()"),
                    ])
                    .font_size(13.5)
                    .foreground_color(Color::rgb(201, 201, 207))
                    .line_height(23.625)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("div-363"),
                    rich_text([
                        run(".fetcher("),
                        run("Allowlist").foreground_color(Color::rgb(233, 228, 212)),
                        run("::offline(net).allow("),
                        run("\"docs.example.com\"").foreground_color(Color::rgb(169, 212, 155)),
                        run("))"),
                    ])
                    .font_size(13.5)
                    .foreground_color(Color::rgb(201, 201, 207))
                    .line_height(23.625)
                    .padding_edge(Edge::Leading, 28.0)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("div-366"),
                    rich_text([
                        run(".jitless("),
                        run("true").foreground_color(Color::rgb(242, 184, 142)),
                        run(")"),
                    ])
                    .font_size(13.5)
                    .foreground_color(Color::rgb(201, 201, 207))
                    .line_height(23.625)
                    .padding_edge(Edge::Leading, 28.0)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("div-369"),
                    text(".build();")
                    .font_size(13.5)
                    .foreground_color(Color::rgb(201, 201, 207))
                    .line_height(23.625)
                    .padding_edge(Edge::Leading, 28.0)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("div-371"),
                    rich_text([
                        run("let").foreground_color(Color::rgb(240, 106, 76)),
                        run(" host = "),
                        run("EngineHost").foreground_color(Color::rgb(233, 228, 212)),
                        run("::inline(engine);"),
                    ])
                    .font_size(13.5)
                    .foreground_color(Color::rgb(201, 201, 207))
                    .line_height(23.625)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("div-372"),
                    rich_text([
                        run("let").foreground_color(Color::rgb(240, 106, 76)),
                        run(" view = host.open("),
                        run("Url").foreground_color(Color::rgb(233, 228, 212)),
                        run("::parse("),
                        run("\"https://docs.example.com/\"").foreground_color(Color::rgb(169, 212, 155)),
                        run(")?);"),
                    ])
                    .font_size(13.5)
                    .foreground_color(Color::rgb(201, 201, 207))
                    .line_height(23.625)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("div-375"),
                    text("// a script and memory budget for this view")
                    .font_size(13.5)
                    .foreground_color(Color::rgb(110, 110, 120))
                    .line_height(23.625)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("div-379"),
                    rich_text([
                        run("host.act(view, "),
                        run("Act").foreground_color(Color::rgb(233, 228, 212)),
                        run("::"),
                        run("Budgets").foreground_color(Color::rgb(233, 228, 212)),
                        run("("),
                        run("Budgets").foreground_color(Color::rgb(233, 228, 212)),
                        run(" {"),
                    ])
                    .font_size(13.5)
                    .foreground_color(Color::rgb(201, 201, 207))
                    .line_height(23.625)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("div-380"),
                    rich_text([
                        run("script: "),
                        run("Some").foreground_color(Color::rgb(233, 228, 212)),
                        run("("),
                        run("50_000_000").foreground_color(Color::rgb(242, 184, 142)),
                        run("),"),
                    ])
                    .font_size(13.5)
                    .foreground_color(Color::rgb(201, 201, 207))
                    .line_height(23.625)
                    .padding_edge(Edge::Leading, 28.0)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("div-384"),
                    rich_text([
                        run("memory: "),
                        run("Some").foreground_color(Color::rgb(233, 228, 212)),
                        run("("),
                        run("256").foreground_color(Color::rgb(242, 184, 142)),
                        run(" << "),
                        run("20").foreground_color(Color::rgb(242, 184, 142)),
                        run("),"),
                    ])
                    .font_size(13.5)
                    .foreground_color(Color::rgb(201, 201, 207))
                    .line_height(23.625)
                    .padding_edge(Edge::Leading, 28.0)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("div-387"),
                    text("}));")
                    .font_size(13.5)
                    .foreground_color(Color::rgb(201, 201, 207))
                    .line_height(23.625)
                    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                    .test_id("div-391"),
                )
                .alignment(HorizontalAlignment::Leading)
                .padding_edge(Edge::Top, 20.0)
                .padding_edge(Edge::Trailing, 22.0)
                .padding_edge(Edge::Bottom, 24.0)
                .padding_edge(Edge::Leading, 22.0)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
                .clipped()
                .test_id("div-362"),
            )
            .alignment(HorizontalAlignment::Leading)
            .padding_length(1.0)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
            .background_color(Color::rgb(10, 12, 18))
            .corner_radius(18.0)
            .border(Color::rgba(255, 255, 255, 23), 1.0)
            .clipped()
            .test_id("div-356"),
        )
        .spacing(48.0)
        .alignment(VerticalAlignment::Center)
        .wrapping()
        .line_spacing(48.0)
        .padding_length(49.0)
        .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading)
        .background_gradient(Gradient::linear(Color::rgba(255, 255, 255, 15), Color::rgba(255, 255, 255, 4)).direction(UnitPoint { x: 0.3436, y: -0.2053 }, UnitPoint { x: 0.6564, y: 1.2053 }))
        .corner_radius(32.0)
        .border(Color::rgba(255, 255, 255, 26), 1.0)
        .test_id("div-331")
        .padding_edge(Edge::Top, 140.0)
        .padding_edge(Edge::Trailing, 32.0)
        .padding_edge(Edge::Leading, 32.0)
        .frame_max(1264.0, f64::MAX, Alignment::TopLeading)
        .test_id("section-330")
        .padding_edge(Edge::Leading, 88.0)
        .padding_edge(Edge::Trailing, 88.0),
        vstack!(
            hstack!(
                vstack!(
                    image(picture_hop_logo_light_0())
                    .resizable()
                    .frame_height_aligned(36.0, Alignment::TopLeading)
                    .clipped()
                    .test_id("img-397"),
                    text("The web, hand-made — in Rust, by The Bunny Lab.")
                    .font_size(15.0)
                    .foreground_color(Color::rgb(154, 154, 161))
                    .line_height(23.25)
                    .frame_width_aligned(360.0, Alignment::TopLeading)
                    .test_id("p-398"),
                )
                .spacing(12.0)
                .alignment(HorizontalAlignment::Leading)
                .test_id("div-396"),
                spacer(),
                hstack!(
                    vstack!(
                        text("BROWSER")
                        .font_size(12.0)
                        .foreground_color(Color::rgb(142, 142, 150))
                        .tracking(0.96)
                        .line_height(18.6)
                        .frame_width_aligned(118.875, Alignment::TopLeading)
                        .test_id("span-401"),
                        text("Download")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(201, 201, 207))
                        .line_height(23.25)
                        .frame_width_aligned(118.875, Alignment::TopLeading)
                        .test_id("a-402")
                        .on_click(|| {}),
                        text("Try it on the web")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(201, 201, 207))
                        .line_height(23.25)
                        .frame_width_aligned(118.875, Alignment::TopLeading)
                        .test_id("a-403")
                        .on_click(|| {}),
                        text("Features")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(201, 201, 207))
                        .line_height(23.25)
                        .frame_width_aligned(118.875, Alignment::TopLeading)
                        .test_id("a-404")
                        .on_click(|| {}),
                    )
                    .spacing(10.0)
                    .alignment(HorizontalAlignment::Leading)
                    .frame_height_aligned(118.3438, Alignment::TopLeading)
                    .test_id("div-400"),
                    vstack!(
                        text("COMPANY")
                        .font_size(12.0)
                        .foreground_color(Color::rgb(142, 142, 150))
                        .tracking(0.96)
                        .line_height(18.6)
                        .frame_width_aligned(80.1563, Alignment::TopLeading)
                        .test_id("span-406"),
                        text("Embedding")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(201, 201, 207))
                        .line_height(23.25)
                        .frame_width_aligned(80.1563, Alignment::TopLeading)
                        .test_id("a-407")
                        .on_click(|| {}),
                        text("Contact")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(201, 201, 207))
                        .line_height(23.25)
                        .frame_width_aligned(80.1563, Alignment::TopLeading)
                        .test_id("a-408")
                        .on_click(|| {}),
                        text("Privacy")
                        .font_size(15.0)
                        .foreground_color(Color::rgb(201, 201, 207))
                        .line_height(23.25)
                        .frame_width_aligned(80.1563, Alignment::TopLeading)
                        .test_id("a-409")
                        .on_click(|| {}),
                    )
                    .spacing(10.0)
                    .alignment(HorizontalAlignment::Leading)
                    .frame_height_aligned(118.3438, Alignment::TopLeading)
                    .test_id("div-405"),
                )
                .spacing(48.0)
                .alignment(VerticalAlignment::Top)
                .wrapping()
                .line_spacing(48.0)
                .test_id("nav-399"),
            )
            .spacing(64.0)
            .alignment(VerticalAlignment::Top)
            .wrapping()
            .line_spacing(32.0)
            .frame_max(1200.0, f64::MAX, Alignment::TopLeading)
            .test_id("div-395")
            .padding_edge(Edge::Leading, 88.0)
            .padding_edge(Edge::Trailing, 88.0),
            text("© 2026 The Bunny Lab. hop is free to use.")
            .font_size(13.0)
            .foreground_color(Color::rgb(142, 142, 150))
            .line_height(20.15)
            .padding_edge(Edge::Top, 25.0)
            .frame_max(1200.0, f64::MAX, Alignment::TopLeading)
            .overlay(UnitPoint::TOP, spacer().frame_height(1.0).background_color(Color::rgba(255, 255, 255, 20)))
            .test_id("p-410")
            .padding_edge(Edge::Top, 40.0)
            .padding_edge(Edge::Leading, 88.0)
            .padding_edge(Edge::Trailing, 88.0),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding_edge(Edge::Top, 40.0)
        .padding_edge(Edge::Trailing, 32.0)
        .padding_edge(Edge::Bottom, 48.0)
        .padding_edge(Edge::Leading, 32.0)
        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
        .background_color(Color::rgb(17, 23, 34))
        .test_id("div-394")
        .padding_edge(Edge::Top, 200.0)
        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
        .overlay(UnitPoint::TOP_LEADING, spacer()
            .frame_aligned(1440.0, 200.0, Alignment::TopLeading)
            .clipped()
            .test_id("svg-393"))
        .test_id("footer-392")
        .padding_edge(Edge::Top, 160.0),
    )
    .alignment(HorizontalAlignment::Leading)
    .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
    .background_color(Color::rgb(11, 15, 23))
    .clipped()
    .test_id("div-1")
    .frame_aligned(1440.0, 5690.1328, Alignment::TopLeading)
    .test_id("body-0")
    .frame_aligned(1440.0, 5691.0, Alignment::TopLeading)
    .font_family("Times")
    .font_size(16.0)
    .foreground_color(Color::rgb(0, 0, 0))
}
