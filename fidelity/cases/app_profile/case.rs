//! Written by `bunny import html` from apps/profile.html.

use crate::prelude::*;

fn picture_album_4() -> ImageSource {
    ImageSource::from_bytes(include_bytes!("../../assets/album.png").as_slice())
}

fn picture_avatar_1() -> ImageSource {
    ImageSource::from_bytes(include_bytes!("../../assets/avatar.png").as_slice())
}

fn picture_cover_0() -> ImageSource {
    ImageSource::from_bytes(include_bytes!("../../assets/cover.png").as_slice())
}

fn picture_photo1_3() -> ImageSource {
    ImageSource::from_bytes(include_bytes!("../../assets/photo1.png").as_slice())
}

fn picture_photo2_2() -> ImageSource {
    ImageSource::from_bytes(include_bytes!("../../assets/photo2.png").as_slice())
}

fn picture_photo3_5() -> ImageSource {
    ImageSource::from_bytes(include_bytes!("../../assets/photo3.png").as_slice())
}

#[must_use]
pub fn view() -> impl View {
    vstack!(
        image(picture_cover_0())
        .resizable()
        .aspect_ratio(ContentMode::Fill)
        .clipped()
        .frame_aligned(390.0, 130.0, Alignment::TopLeading)
        .clipped()
        .test_id("img-2")
        .frame_height_aligned(130.0, Alignment::TopLeading)
        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
        .overlay(UnitPoint::TOP_LEADING, hstack!(
                text("9:41")
                .font_size(12.0)
                .font_weight(Weight::Bold)
                .foreground_color(Color::rgb(255, 255, 255))
                .frame_max(f64::MAX, f64::INFINITY, Alignment::TopLeading)
                .test_id("span-4"),
                spacer(),
                text("5G 100%")
                .font_size(12.0)
                .font_weight(Weight::Bold)
                .foreground_color(Color::rgb(255, 255, 255))
                .frame_max(f64::MAX, f64::INFINITY, Alignment::TopLeading)
                .test_id("span-5"),
            )
            .alignment(VerticalAlignment::Top)
            .frame_aligned(354.0, 14.5, Alignment::TopLeading)
            .test_id("div.status-3")
            .offset(18.0, 8.0))
        .overlay(UnitPoint::TOP_LEADING, spacer()
            .frame_aligned(390.0, 130.0, Alignment::TopLeading)
            .background_gradient(Gradient::Linear { start: UnitPoint { x: 0.5, y: 0.0 }, end: UnitPoint { x: 0.5, y: 1.0 }, from: Color::rgba(0, 0, 0, 89), to: Color::rgba(124, 58, 237, 89) }))
        .test_id("div.cover-1"),
        hstack!(
            image(picture_avatar_1())
            .resizable()
            .aspect_ratio(ContentMode::Fill)
            .clipped()
            .padding_length(4.0)
            .frame_aligned(92.0, 92.0, Alignment::TopLeading)
            .corner_radius(46.0)
            .border(Color::rgb(255, 255, 255), 4.0)
            .shadow_color(14.0, Color::rgba(0, 0, 0, 64))
            .shadow_offset(0.0, 4.0)
            .clipped()
            .test_id("img.avatar-7")
            .padding_edge(Edge::Top, -48.0),
            image(picture_photo2_2())
            .resizable()
            .aspect_ratio(ContentMode::Fill)
            .clipped()
            .padding_length(2.0)
            .frame_aligned(14.875, 34.0, Alignment::TopLeading)
            .corner_radius(7.4375)
            .border(Color::rgb(255, 255, 255), 2.0)
            .clipped()
            .test_id("img-9")
            .padding_length(3.0)
            .frame_aligned(20.875, 40.0, Alignment::TopLeading)
            .corner_radius(10.4375)
            .test_id("div.ring-8")
            .padding_edge(Edge::Bottom, 4.0),
            spacer().frame_height(0.0)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
            .test_id("div.sp-10"),
            hstack!(
                hstack!(
                    text("Follow")
                    .font_size(13.0)
                    .font_weight(Weight::Bold)
                    .foreground_color(Color::rgb(255, 255, 255)),
                )
                .alignment(VerticalAlignment::Center)
                .padding_edge(Edge::Trailing, 16.0)
                .padding_edge(Edge::Leading, 16.0)
                .frame_height_aligned(34.0, Alignment::Leading)
                .background_gradient(Gradient::Linear { start: UnitPoint { x: 0.1285, y: -0.2644 }, end: UnitPoint { x: 0.8715, y: 1.2644 }, from: Color::rgb(124, 58, 237), to: Color::rgb(236, 72, 153) })
                .corner_radius(17.0)
                .shadow_color(8.0, Color::rgba(124, 58, 237, 89))
                .shadow_offset(0.0, 3.0)
                .test_id("span.b-12"),
                hstack!(
                    text("Message")
                    .font_size(13.0)
                    .font_weight(Weight::Bold)
                    .foreground_color(Color::rgb(124, 58, 237)),
                )
                .alignment(VerticalAlignment::Center)
                .padding_edge(Edge::Top, 1.5)
                .padding_edge(Edge::Trailing, 17.5)
                .padding_edge(Edge::Bottom, 1.5)
                .padding_edge(Edge::Leading, 17.5)
                .frame_height_aligned(34.0, Alignment::Leading)
                .corner_radius(17.0)
                .border(Color::rgb(124, 58, 237), 1.5)
                .test_id("span.b-13"),
                hstack!(
                    spacer()
                    .frame_aligned(4.0, 4.0, Alignment::TopLeading)
                    .background_color(Color::rgb(85, 85, 92))
                    .corner_radius(2.0)
                    .test_id("i-15"),
                    spacer()
                    .frame_aligned(4.0, 4.0, Alignment::TopLeading)
                    .background_color(Color::rgb(85, 85, 92))
                    .corner_radius(2.0)
                    .test_id("i-16"),
                    spacer()
                    .frame_aligned(4.0, 4.0, Alignment::TopLeading)
                    .background_color(Color::rgb(85, 85, 92))
                    .corner_radius(2.0)
                    .test_id("i-17"),
                )
                .spacing(3.0)
                .alignment(VerticalAlignment::Center)
                .padding_length(1.5)
                .frame_aligned(34.0, 34.0, Alignment::Center)
                .corner_radius(17.0)
                .border(Color::rgb(212, 212, 216), 1.5)
                .test_id("span.b-14"),
            )
            .spacing(8.0)
            .alignment(VerticalAlignment::Top)
            .padding_edge(Edge::Bottom, 4.0)
            .test_id("div.actions-11"),
        )
        .spacing(12.0)
        .alignment(VerticalAlignment::Bottom)
        .padding_edge(Edge::Trailing, 18.0)
        .padding_edge(Edge::Leading, 18.0)
        .frame_max(f64::INFINITY, f64::MAX, Alignment::BottomLeading)
        .test_id("div.head-6"),
        vstack!(
            hstack!(
                text("Nora Vale")
                .font_size(20.0)
                .font_weight(Weight::Black)
                .tracking(-0.4),
                spacer()
                .frame_aligned(16.0, 16.0, Alignment::TopLeading)
                .background_color(Color::rgb(37, 99, 235))
                .corner_radius(8.0)
                .overlay(UnitPoint::TOP_LEADING, spacer()
                    .padding_edge(Edge::Bottom, 2.0)
                    .padding_edge(Edge::Leading, 2.0)
                    .frame_aligned(7.0, 4.0, Alignment::TopLeading)
                    .overlay(UnitPoint::BOTTOM, spacer().frame_height(2.0).background_color(Color::rgb(255, 255, 255)))
                    .overlay(UnitPoint::LEADING, spacer().frame_width(2.0).background_color(Color::rgb(255, 255, 255)))
                    .offset(4.5, 3.5))
                .test_id("span.badge-20"),
            )
            .spacing(6.0)
            .alignment(VerticalAlignment::Center)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::Leading)
            .test_id("div.name-19"),
            text("@noravale • Lisbon, PT")
            .font_size(13.0)
            .foreground_color(Color::rgb(107, 107, 115))
            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
            .test_id("div.handle-21"),
            rich_text([
                run("Photographer & "),
                run("visual storyteller").weight(Weight::Bold),
                run(". Chasing "),
                run("golden hour").italic(),
                run(" since 2014. Prints at "),
                run("noravale.studio").foreground_color(Color::rgb(124, 58, 237)).underline(None),
                run(" "),
                run("#analog").weight(Weight::Bold).foreground_color(Color::rgb(124, 58, 237)),
                run(" "),
                run("#travel").weight(Weight::Bold).foreground_color(Color::rgb(124, 58, 237)),
                run(" — shot with "),
                run("35mm f/1.4").font_size(12.0).foreground_color(Color::rgb(190, 24, 93)).background_color(Color::rgb(241, 240, 245)),
                run(". "),
                run("Open for commissions").foreground_color(Color::rgb(0, 0, 0)).background_color(Color::rgb(253, 230, 138)),
                run(" this autumn."),
            ])
            .font_size(13.0)
            .foreground_color(Color::rgb(43, 43, 48))
            .line_height(18.85)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
            .test_id("p.bio-22")
            .padding_edge(Edge::Top, 6.0),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding_edge(Edge::Top, 6.0)
        .padding_edge(Edge::Trailing, 18.0)
        .padding_edge(Edge::Leading, 18.0)
        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
        .test_id("div.id-18"),
        hstack!(
            vstack!(
                text("1,284")
                .font_size(17.0)
                .font_weight(Weight::Bold)
                .tracking(-0.17)
                .multiline_text_alignment(TextAlignment::Center)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::Top)
                .test_id("b-32"),
                rich_text([run("Posts").font_size(11.0).foreground_color(Color::rgb(107, 107, 115))])
                .multiline_text_alignment(TextAlignment::Center)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::Center),
            )
            .alignment(HorizontalAlignment::Leading)
            .padding_edge(Edge::Trailing, 1.0)
            .frame_height_aligned(40.0, Alignment::Top)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::Top)
            .overlay(UnitPoint::TRAILING, spacer().frame_width(1.0).background_color(Color::rgb(228, 228, 231)))
            .test_id("div-31"),
            vstack!(
                text("48.2K")
                .font_size(17.0)
                .font_weight(Weight::Bold)
                .tracking(-0.17)
                .multiline_text_alignment(TextAlignment::Center)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::Top)
                .test_id("b-35"),
                rich_text([run("Followers").font_size(11.0).foreground_color(Color::rgb(107, 107, 115))])
                .multiline_text_alignment(TextAlignment::Center)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::Center),
            )
            .alignment(HorizontalAlignment::Leading)
            .padding_edge(Edge::Trailing, 1.0)
            .frame_height_aligned(40.0, Alignment::Top)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::Top)
            .overlay(UnitPoint::TRAILING, spacer().frame_width(1.0).background_color(Color::rgb(228, 228, 231)))
            .test_id("div-34"),
            vstack!(
                text("612")
                .font_size(17.0)
                .font_weight(Weight::Bold)
                .tracking(-0.17)
                .multiline_text_alignment(TextAlignment::Center)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::Top)
                .test_id("b-38"),
                rich_text([run("Following").font_size(11.0).foreground_color(Color::rgb(107, 107, 115))])
                .multiline_text_alignment(TextAlignment::Center)
                .frame_max(f64::INFINITY, f64::MAX, Alignment::Center),
            )
            .alignment(HorizontalAlignment::Leading)
            .frame_height_aligned(40.0, Alignment::Top)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::Top)
            .test_id("div-37"),
        )
        .alignment(VerticalAlignment::Top)
        .padding_edge(Edge::Top, 9.0)
        .padding_edge(Edge::Bottom, 9.0)
        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
        .overlay(UnitPoint::TOP, spacer().frame_height(1.0).background_color(Color::rgb(238, 238, 238)))
        .overlay(UnitPoint::BOTTOM, spacer().frame_height(1.0).background_color(Color::rgb(238, 238, 238)))
        .test_id("div.stats-30")
        .padding_edge(Edge::Top, 10.0)
        .padding_edge(Edge::Leading, 18.0)
        .padding_edge(Edge::Trailing, 18.0),
        hstack!(
            text("Photos")
            .font_size(13.0)
            .font_weight(Weight::Bold)
            .multiline_text_alignment(TextAlignment::Center)
            .padding_edge(Edge::Top, 9.0)
            .padding_edge(Edge::Bottom, 9.0)
            .frame_height_aligned(34.0, Alignment::Top)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::Top)
            .overlay(UnitPoint::TOP_LEADING, spacer()
                .frame_aligned(44.25, 3.0, Alignment::Top)
                .background_color(Color::rgb(124, 58, 237))
                .corner_radius(Corners { top_left: 1.5, top_right: 1.5, bottom_right: 0.0, bottom_left: 0.0 })
                .offset(22.125, 31.0))
            .test_id("span.tab-41"),
            text("Reels")
            .font_size(13.0)
            .font_weight(Weight::Bold)
            .foreground_color(Color::rgb(138, 138, 147))
            .multiline_text_alignment(TextAlignment::Center)
            .padding_edge(Edge::Top, 9.0)
            .padding_edge(Edge::Bottom, 9.0)
            .frame_height_aligned(34.0, Alignment::Top)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::Top)
            .test_id("span.tab-42"),
            text("Tagged")
            .font_size(13.0)
            .font_weight(Weight::Bold)
            .foreground_color(Color::rgb(138, 138, 147))
            .multiline_text_alignment(TextAlignment::Center)
            .padding_edge(Edge::Top, 9.0)
            .padding_edge(Edge::Bottom, 9.0)
            .frame_height_aligned(34.0, Alignment::Top)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::Top)
            .test_id("span.tab-43"),
            text("Saved")
            .font_size(13.0)
            .font_weight(Weight::Bold)
            .foreground_color(Color::rgb(138, 138, 147))
            .multiline_text_alignment(TextAlignment::Center)
            .padding_edge(Edge::Top, 9.0)
            .padding_edge(Edge::Bottom, 9.0)
            .frame_height_aligned(34.0, Alignment::Top)
            .frame_max(f64::INFINITY, f64::MAX, Alignment::Top)
            .test_id("span.tab-44"),
        )
        .alignment(VerticalAlignment::Top)
        .padding_edge(Edge::Trailing, 18.0)
        .padding_edge(Edge::Leading, 18.0)
        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
        .test_id("div.tabs-40")
        .padding_edge(Edge::Top, 4.0),
        text("Latest series: “Salt & Light” — twelve frames from a month along the Atlantic coast, developed by hand in a small darkroom and scanned on a drum scanner for extra grain and depth.")
        .font_size(12.0)
        .foreground_color(Color::rgb(85, 85, 92))
        .line_height(16.2)
        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
        .clipped()
        .test_id("p.caption-45")
        .padding_edge(Edge::Top, 6.0)
        .padding_edge(Edge::Leading, 18.0)
        .padding_edge(Edge::Trailing, 18.0),
        hstack!(
            vstack!(
                image(picture_photo1_3())
                .resizable()
                .aspect_ratio(ContentMode::Fill)
                .clipped()
                .frame_aligned(220.3281, 220.3281, Alignment::TopLeading)
                .clipped()
                .test_id("img-49")
                .frame_aligned(220.3281, 220.3281, Alignment::TopLeading)
                .background_color(Color::rgb(221, 221, 221))
                .corner_radius(4.0)
                .clipped()
                .test_id("div.t-48"),
                hstack!(
                    image(picture_album_4())
                    .resizable()
                    .aspect_ratio(ContentMode::Fill)
                    .clipped()
                    .frame_aligned(108.6641, 108.6641, Alignment::TopLeading)
                    .clipped()
                    .test_id("img-55")
                    .frame_aligned(108.6641, 108.6641, Alignment::TopLeading)
                    .background_color(Color::rgb(221, 221, 221))
                    .corner_radius(4.0)
                    .clipped()
                    .test_id("div.t-54"),
                    image(picture_photo1_3())
                    .resizable()
                    .aspect_ratio(ContentMode::Fill)
                    .clipped()
                    .frame_aligned(108.6641, 108.6641, Alignment::TopLeading)
                    .clipped()
                    .test_id("img-57")
                    .frame_aligned(108.6641, 108.6641, Alignment::TopLeading)
                    .background_color(Color::rgb(221, 221, 221))
                    .corner_radius(4.0)
                    .clipped()
                    .test_id("div.t-56")
                    .padding_edge(Edge::Leading, 3.0),
                )
                .alignment(VerticalAlignment::Top)
                .padding_edge(Edge::Top, 3.0),
                hstack!(
                    image(picture_photo3_5())
                    .resizable()
                    .aspect_ratio(ContentMode::Fill)
                    .clipped()
                    .frame_aligned(108.6641, 108.6641, Alignment::TopLeading)
                    .clipped()
                    .test_id("img-63")
                    .frame_aligned(108.6641, 108.6641, Alignment::TopLeading)
                    .background_color(Color::rgb(221, 221, 221))
                    .corner_radius(4.0)
                    .clipped()
                    .test_id("div.t-62"),
                    image(picture_album_4())
                    .resizable()
                    .aspect_ratio(ContentMode::Fill)
                    .clipped()
                    .frame_aligned(108.6641, 108.6641, Alignment::TopLeading)
                    .clipped()
                    .test_id("img-65")
                    .frame_aligned(108.6641, 108.6641, Alignment::TopLeading)
                    .background_color(Color::rgb(221, 221, 221))
                    .corner_radius(4.0)
                    .clipped()
                    .test_id("div.t-64")
                    .padding_edge(Edge::Leading, 3.0),
                )
                .alignment(VerticalAlignment::Top)
                .padding_edge(Edge::Top, 3.0),
            )
            .alignment(HorizontalAlignment::Leading),
            vstack!(
                image(picture_photo2_2())
                .resizable()
                .aspect_ratio(ContentMode::Fill)
                .clipped()
                .frame_aligned(108.6641, 108.6641, Alignment::TopLeading)
                .clipped()
                .test_id("img-51")
                .frame_aligned(108.6641, 108.6641, Alignment::TopLeading)
                .background_color(Color::rgb(221, 221, 221))
                .corner_radius(4.0)
                .clipped()
                .test_id("div.t-50"),
                image(picture_photo3_5())
                .resizable()
                .aspect_ratio(ContentMode::Fill)
                .clipped()
                .frame_aligned(108.6641, 108.6641, Alignment::TopLeading)
                .clipped()
                .test_id("img-53")
                .frame_aligned(108.6641, 108.6641, Alignment::TopLeading)
                .background_color(Color::rgb(221, 221, 221))
                .corner_radius(4.0)
                .clipped()
                .test_id("div.t-52")
                .padding_edge(Edge::Top, 3.0),
                image(picture_photo2_2())
                .resizable()
                .aspect_ratio(ContentMode::Fill)
                .clipped()
                .frame_aligned(108.6641, 108.6641, Alignment::TopLeading)
                .clipped()
                .test_id("img-59")
                .frame_aligned(108.6641, 108.6641, Alignment::TopLeading)
                .background_color(Color::rgb(221, 221, 221))
                .corner_radius(4.0)
                .clipped()
                .overlay(UnitPoint::TOP_LEADING, spacer()
                    .padding_edge(Edge::Top, 10.0)
                    .padding_edge(Edge::Bottom, 10.0)
                    .padding_edge(Edge::Leading, 16.0)
                    .frame_aligned(16.0, 20.0, Alignment::TopLeading)
                    .overlay(UnitPoint::TOP, spacer().frame_height(10.0).background_color(Color::rgba(0, 0, 0, 0)))
                    .overlay(UnitPoint::BOTTOM, spacer().frame_height(10.0).background_color(Color::rgba(0, 0, 0, 0)))
                    .overlay(UnitPoint::LEADING, spacer().frame_width(16.0).background_color(Color::rgba(255, 255, 255, 242)))
                    .test_id("span.play-60")
                    .offset(-5.6, -10.0)
                    .offset(54.3281, 54.3281))
                .overlay(UnitPoint::TOP_LEADING, text("0:24")
                    .font_size(10.0)
                    .font_weight(Weight::Bold)
                    .foreground_color(Color::rgb(255, 255, 255))
                    .padding_edge(Edge::Top, 1.0)
                    .padding_edge(Edge::Trailing, 6.0)
                    .padding_edge(Edge::Bottom, 1.0)
                    .padding_edge(Edge::Leading, 6.0)
                    .frame_aligned(31.9219, 14.0, Alignment::TopLeading)
                    .background_color(Color::rgba(0, 0, 0, 166))
                    .corner_radius(7.0)
                    .test_id("span.dur-61")
                    .offset(72.7422, 90.6641))
                .test_id("div.t-58")
                .padding_edge(Edge::Top, 3.0),
            )
            .alignment(HorizontalAlignment::Leading)
            .padding_edge(Edge::Leading, 3.0),
        )
        .alignment(VerticalAlignment::Top)
        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
        .test_id("div.grid-47")
        .padding_edge(Edge::Trailing, 22.0)
        .frame_max(f64::INFINITY, f64::MAX, Alignment::TopLeading)
        .overlay(UnitPoint::TOP_LEADING, text("PORTFOLIO 2026")
            .font_size(10.0)
            .foreground_color(Color::rgb(138, 138, 147))
            .tracking(2.5)
            .multiline_text_alignment(TextAlignment::Center)
            .frame_aligned(18.0, 443.6563, Alignment::Top)
            .background_color(Color::rgb(246, 245, 250))
            .corner_radius(9.0)
            .test_id("div.rail-66")
            .offset(336.0, 0.0))
        .test_id("div.gridwrap-46")
        .padding_edge(Edge::Top, 6.0)
        .padding_edge(Edge::Leading, 18.0)
        .padding_edge(Edge::Trailing, 18.0),
    )
    .alignment(HorizontalAlignment::Leading)
    .frame_aligned(390.0, 844.0, Alignment::TopLeading)
    .background_color(Color::rgb(255, 255, 255))
    .clipped()
    .overlay(UnitPoint::TOP_LEADING, hstack!(
            spacer(),
            spacer()
            .padding_length(2.0)
            .frame_aligned(22.0, 22.0, Alignment::TopLeading)
            .background_color(Color::rgb(124, 58, 237))
            .corner_radius(6.0)
            .border(Color::rgb(124, 58, 237), 2.0)
            .test_id("i.on-68"),
            spacer(),
            spacer()
            .padding_length(2.0)
            .frame_aligned(22.0, 22.0, Alignment::TopLeading)
            .corner_radius(11.0)
            .border(Color::rgb(138, 138, 147), 2.0)
            .test_id("i.r-69"),
            spacer(),
            spacer()
            .padding_length(2.0)
            .frame_aligned(22.0, 22.0, Alignment::TopLeading)
            .corner_radius(6.0)
            .border(Color::rgb(138, 138, 147), 2.0)
            .test_id("i-70"),
            spacer(),
            spacer()
            .padding_length(2.0)
            .frame_aligned(22.0, 22.0, Alignment::TopLeading)
            .corner_radius(11.0)
            .border(Color::rgb(138, 138, 147), 2.0)
            .test_id("i.r-71"),
            spacer(),
            spacer()
            .padding_length(2.0)
            .frame_aligned(22.0, 22.0, Alignment::TopLeading)
            .corner_radius(6.0)
            .border(Color::rgb(138, 138, 147), 2.0)
            .test_id("i-72"),
            spacer(),
        )
        .alignment(VerticalAlignment::Center)
        .padding_edge(Edge::Top, 1.0)
        .frame_aligned(390.0, 52.0, Alignment::Leading)
        .background_color(Color::rgb(255, 255, 255))
        .overlay(UnitPoint::TOP, spacer().frame_height(1.0).background_color(Color::rgb(238, 238, 238)))
        .test_id("div.nav-67")
        .offset(0.0, 792.0))
    .test_id("body-0")
    .frame_aligned(390.0, 844.0, Alignment::TopLeading)
    .font_family("Fid Sans")
    .font_size(16.0)
    .foreground_color(Color::rgb(29, 29, 31))
}
