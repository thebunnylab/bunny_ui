//! The house set — sixteen glyphs of generic UI furniture, drawn by
//! hand on the grid. Stroke 2, round pen, the metrics of one family.
//!
//! These are the glyphs a shell needs before it has any art of its
//! own: chevrons, marks, a few nouns. An app's converted icons are the
//! SAME type — the house set only saves the first hour.
//!
//! They also earn their keep as fixtures: the raster tests walk this
//! table, so every shipped drawing is exercised on every target.

use super::{Draw, Glyph, Paint, Rule, Symbol};
use super::Verb::{Close, Cubic, Line, Move};

const STROKE: Paint = Paint::Stroke { width: 2.0 };
const FILL: Paint = Paint::Fill(Rule::NonZero);

// MARK: - Chevrons

const CHEVRON_RIGHT_GLYPH: Glyph = Glyph {
    draws: &[Draw { paint: STROKE, path: &[Move(9.0, 6.0), Line(15.0, 12.0), Line(9.0, 18.0)], tint: None }],
};
/// `›` — disclosure, breadcrumbs, "go".
pub const CHEVRON_RIGHT: Symbol = Symbol::new("chevron.right", &CHEVRON_RIGHT_GLYPH);

const CHEVRON_LEFT_GLYPH: Glyph = Glyph {
    draws: &[Draw { paint: STROKE, path: &[Move(15.0, 6.0), Line(9.0, 12.0), Line(15.0, 18.0)], tint: None }],
};
/// `‹` — back.
pub const CHEVRON_LEFT: Symbol = Symbol::new("chevron.left", &CHEVRON_LEFT_GLYPH);

const CHEVRON_DOWN_GLYPH: Glyph = Glyph {
    draws: &[Draw { paint: STROKE, path: &[Move(6.0, 9.0), Line(12.0, 15.0), Line(18.0, 9.0)], tint: None }],
};
/// `▾` — an open disclosure, a dropdown.
pub const CHEVRON_DOWN: Symbol = Symbol::new("chevron.down", &CHEVRON_DOWN_GLYPH);

const CHEVRON_UP_GLYPH: Glyph = Glyph {
    draws: &[Draw { paint: STROKE, path: &[Move(6.0, 15.0), Line(12.0, 9.0), Line(18.0, 15.0)], tint: None }],
};
/// `▴` — collapse.
pub const CHEVRON_UP: Symbol = Symbol::new("chevron.up", &CHEVRON_UP_GLYPH);

// MARK: - Marks

const CHECK_GLYPH: Glyph = Glyph {
    draws: &[Draw { paint: STROKE, path: &[Move(5.0, 12.5), Line(10.0, 17.5), Line(19.0, 7.0)], tint: None }],
};
/// Done, selected, on.
pub const CHECK: Symbol = Symbol::new("check", &CHECK_GLYPH);

const CLOSE_GLYPH: Glyph = Glyph {
    draws: &[Draw {
        paint: STROKE,
        path: &[Move(6.0, 6.0), Line(18.0, 18.0), Move(18.0, 6.0), Line(6.0, 18.0)], tint: None,
    }],
};
/// `✕` — dismiss, the tab's corner.
pub const CLOSE: Symbol = Symbol::new("close", &CLOSE_GLYPH);

const PLUS_GLYPH: Glyph = Glyph {
    draws: &[Draw {
        paint: STROKE,
        path: &[Move(12.0, 5.0), Line(12.0, 19.0), Move(5.0, 12.0), Line(19.0, 12.0)], tint: None,
    }],
};
/// Add.
pub const PLUS: Symbol = Symbol::new("plus", &PLUS_GLYPH);

const MINUS_GLYPH: Glyph =
    Glyph { draws: &[Draw { paint: STROKE, path: &[Move(5.0, 12.0), Line(19.0, 12.0)], tint: None }] };
/// Remove, collapse.
pub const MINUS: Symbol = Symbol::new("minus", &MINUS_GLYPH);

// MARK: - Nouns

const SEARCH_GLYPH: Glyph = Glyph {
    draws: &[Draw {
        paint: STROKE,
        path: &[
            // a circle of four cubics (kappa · 6.5 ≈ 3.59)…
            Move(17.5, 11.0),
            Cubic(17.5, 14.59, 14.59, 17.5, 11.0, 17.5),
            Cubic(7.41, 17.5, 4.5, 14.59, 4.5, 11.0),
            Cubic(4.5, 7.41, 7.41, 4.5, 11.0, 4.5),
            Cubic(14.59, 4.5, 17.5, 7.41, 17.5, 11.0),
            Close,
            // …and its handle
            Move(15.8, 15.8),
            Line(20.0, 20.0),
        ], tint: None,
    }],
};
/// `⌕` — find.
pub const SEARCH: Symbol = Symbol::new("search", &SEARCH_GLYPH);

const FOLDER_GLYPH: Glyph = Glyph {
    draws: &[Draw {
        paint: STROKE,
        path: &[
            // the tab shoulder, then round corners all the way (r 2)
            Move(3.0, 7.0),
            Cubic(3.0, 5.9, 3.9, 5.0, 5.0, 5.0),
            Line(8.0, 5.0),
            Line(10.0, 7.0),
            Line(19.0, 7.0),
            Cubic(20.1, 7.0, 21.0, 7.9, 21.0, 9.0),
            Line(21.0, 17.0),
            Cubic(21.0, 18.1, 20.1, 19.0, 19.0, 19.0),
            Line(5.0, 19.0),
            Cubic(3.9, 19.0, 3.0, 18.1, 3.0, 17.0),
            Close,
        ], tint: None,
    }],
};
/// The explorer's noun.
pub const FOLDER: Symbol = Symbol::new("folder", &FOLDER_GLYPH);

const DOCUMENT_GLYPH: Glyph = Glyph {
    draws: &[Draw {
        paint: STROKE,
        path: &[
            // the page, its corner cut by the seal back to the start
            Move(14.0, 3.0),
            Line(7.0, 3.0),
            Cubic(5.9, 3.0, 5.0, 3.9, 5.0, 5.0),
            Line(5.0, 19.0),
            Cubic(5.0, 20.1, 5.9, 21.0, 7.0, 21.0),
            Line(17.0, 21.0),
            Cubic(18.1, 21.0, 19.0, 20.1, 19.0, 19.0),
            Line(19.0, 8.0),
            Close,
            // the fold
            Move(14.0, 3.0),
            Line(14.0, 8.0),
            Line(19.0, 8.0),
        ], tint: None,
    }],
};
/// A file, a page.
pub const DOCUMENT: Symbol = Symbol::new("document", &DOCUMENT_GLYPH);

const SIDEBAR_GLYPH: Glyph = Glyph {
    draws: &[Draw {
        paint: STROKE,
        path: &[
            // the window (r 2)…
            Move(5.0, 5.0),
            Line(19.0, 5.0),
            Cubic(20.1, 5.0, 21.0, 5.9, 21.0, 7.0),
            Line(21.0, 17.0),
            Cubic(21.0, 18.1, 20.1, 19.0, 19.0, 19.0),
            Line(5.0, 19.0),
            Cubic(3.9, 19.0, 3.0, 18.1, 3.0, 17.0),
            Line(3.0, 7.0),
            Cubic(3.0, 5.9, 3.9, 5.0, 5.0, 5.0),
            Close,
            // …and the rail
            Move(9.5, 5.0),
            Line(9.5, 19.0),
        ], tint: None,
    }],
};
/// `▤` — the panel toggle.
pub const SIDEBAR: Symbol = Symbol::new("sidebar", &SIDEBAR_GLYPH);

const GEAR_GLYPH: Glyph = Glyph {
    draws: &[Draw {
        paint: STROKE,
        path: &[
            // the wheel (r 5.5, kappa ≈ 3.04)
            Move(17.5, 12.0),
            Cubic(17.5, 15.04, 15.04, 17.5, 12.0, 17.5),
            Cubic(8.96, 17.5, 6.5, 15.04, 6.5, 12.0),
            Cubic(6.5, 8.96, 8.96, 6.5, 12.0, 6.5),
            Cubic(15.04, 6.5, 17.5, 8.96, 17.5, 12.0),
            Close,
            // the hub (r 2.5)
            Move(14.5, 12.0),
            Cubic(14.5, 13.38, 13.38, 14.5, 12.0, 14.5),
            Cubic(10.62, 14.5, 9.5, 13.38, 9.5, 12.0),
            Cubic(9.5, 10.62, 10.62, 9.5, 12.0, 9.5),
            Cubic(13.38, 9.5, 14.5, 10.62, 14.5, 12.0),
            Close,
            // eight teeth on the diagonals of the clock
            Move(17.5, 12.0),
            Line(20.0, 12.0),
            Move(15.89, 15.89),
            Line(17.66, 17.66),
            Move(12.0, 17.5),
            Line(12.0, 20.0),
            Move(8.11, 15.89),
            Line(6.34, 17.66),
            Move(6.5, 12.0),
            Line(4.0, 12.0),
            Move(8.11, 8.11),
            Line(6.34, 6.34),
            Move(12.0, 6.5),
            Line(12.0, 4.0),
            Move(15.89, 8.11),
            Line(17.66, 6.34),
        ], tint: None,
    }],
};
/// `⚙` — settings.
pub const GEAR: Symbol = Symbol::new("gear", &GEAR_GLYPH);

const INFO_GLYPH: Glyph = Glyph {
    draws: &[
        Draw {
            paint: STROKE,
            path: &[
                // the ring (r 9, kappa ≈ 4.97)
                Move(21.0, 12.0),
                Cubic(21.0, 16.97, 16.97, 21.0, 12.0, 21.0),
                Cubic(7.03, 21.0, 3.0, 16.97, 3.0, 12.0),
                Cubic(3.0, 7.03, 7.03, 3.0, 12.0, 3.0),
                Cubic(16.97, 3.0, 21.0, 7.03, 21.0, 12.0),
                Close,
                // the stem
                Move(12.0, 11.0),
                Line(12.0, 16.5),
            ], tint: None,
        },
        // the dot is a FILLED disc — the second draw of one glyph
        Draw {
            paint: FILL,
            path: &[
                Move(13.2, 7.6),
                Cubic(13.2, 8.26, 12.66, 8.8, 12.0, 8.8),
                Cubic(11.34, 8.8, 10.8, 8.26, 10.8, 7.6),
                Cubic(10.8, 6.94, 11.34, 6.4, 12.0, 6.4),
                Cubic(12.66, 6.4, 13.2, 6.94, 13.2, 7.6),
                Close,
            ], tint: None,
        },
    ],
};
/// `ⓘ`.
pub const INFO: Symbol = Symbol::new("info", &INFO_GLYPH);

const WARNING_GLYPH: Glyph = Glyph {
    draws: &[
        Draw {
            paint: STROKE,
            path: &[
                // the round pen softens the three corners
                Move(12.0, 4.0),
                Line(21.0, 20.0),
                Line(3.0, 20.0),
                Close,
                Move(12.0, 10.0),
                Line(12.0, 14.5),
            ], tint: None,
        },
        Draw {
            paint: FILL,
            path: &[
                Move(13.2, 17.2),
                Cubic(13.2, 17.86, 12.66, 18.4, 12.0, 18.4),
                Cubic(11.34, 18.4, 10.8, 17.86, 10.8, 17.2),
                Cubic(10.8, 16.54, 11.34, 16.0, 12.0, 16.0),
                Cubic(12.66, 16.0, 13.2, 16.54, 13.2, 17.2),
                Close,
            ], tint: None,
        },
    ],
};
/// `⚠` — attention.
pub const WARNING: Symbol = Symbol::new("warning", &WARNING_GLYPH);

const ELLIPSIS_GLYPH: Glyph = Glyph {
    draws: &[Draw {
        paint: FILL,
        path: &[
            // three discs (r 1.6, kappa ≈ 0.88)
            Move(6.6, 12.0),
            Cubic(6.6, 12.88, 5.88, 13.6, 5.0, 13.6),
            Cubic(4.12, 13.6, 3.4, 12.88, 3.4, 12.0),
            Cubic(3.4, 11.12, 4.12, 10.4, 5.0, 10.4),
            Cubic(5.88, 10.4, 6.6, 11.12, 6.6, 12.0),
            Close,
            Move(13.6, 12.0),
            Cubic(13.6, 12.88, 12.88, 13.6, 12.0, 13.6),
            Cubic(11.12, 13.6, 10.4, 12.88, 10.4, 12.0),
            Cubic(10.4, 11.12, 11.12, 10.4, 12.0, 10.4),
            Cubic(12.88, 10.4, 13.6, 11.12, 13.6, 12.0),
            Close,
            Move(20.6, 12.0),
            Cubic(20.6, 12.88, 19.88, 13.6, 19.0, 13.6),
            Cubic(18.12, 13.6, 17.4, 12.88, 17.4, 12.0),
            Cubic(17.4, 11.12, 18.12, 10.4, 19.0, 10.4),
            Cubic(19.88, 10.4, 20.6, 11.12, 20.6, 12.0),
            Close,
        ], tint: None,
    }],
};
/// `…` — more.
pub const ELLIPSIS: Symbol = Symbol::new("ellipsis", &ELLIPSIS_GLYPH);

/// The whole set, for galleries and for the tests that walk it.
pub const ALL: &[Symbol] = &[
    CHEVRON_RIGHT,
    CHEVRON_LEFT,
    CHEVRON_DOWN,
    CHEVRON_UP,
    CHECK,
    CLOSE,
    PLUS,
    MINUS,
    SEARCH,
    FOLDER,
    DOCUMENT,
    SIDEBAR,
    GEAR,
    INFO,
    WARNING,
    ELLIPSIS,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Color;

    #[test]
    fn the_sixteen_keys_never_collide() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a.key, b.key, "{} and {} share a key", a.name, b.name);
            }
        }
    }

    /// Every shipped drawing rasters with ink at the natural sizes and
    /// scales, stays reproducible, and leaves the outer ring bare —
    /// the grid keeps two units of air on every side.
    #[test]
    fn every_house_icon_rasters_and_stays_in_its_box() {
        let ink = Color { r: 0, g: 0, b: 0, a: 255 };
        for symbol in ALL {
            for side in [16usize, 24, 32, 48] {
                let first = super::super::rasterize(symbol.glyph, ink, side, side);
                let inked = first.rgba.chunks_exact(4).filter(|px| px[3] > 0).count();
                assert!(inked > side, "{} is bare at {side}", symbol.name);
                let again = super::super::rasterize(symbol.glyph, ink, side, side);
                assert_eq!(first.rgba, again.rgba, "{} wobbles at {side}", symbol.name);
                // the four corner pixels stay air
                for (x, y) in [(0, 0), (side - 1, 0), (0, side - 1), (side - 1, side - 1)] {
                    let alpha = first.rgba[(y * side + x) * 4 + 3];
                    assert_eq!(alpha, 0, "{} inks the {x},{y} corner at {side}", symbol.name);
                }
            }
        }
    }
}