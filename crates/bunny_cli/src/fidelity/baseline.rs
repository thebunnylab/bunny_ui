//! The ratchet: three numbers per pair of captures per case, kept in a
//! table under version control. A sweep fails when any number grows;
//! `--bless` writes the numbers that shrank.

use std::collections::BTreeMap;
use std::fmt::Write;

/// The three pairs a case is compared in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pair {
    /// Is the element lowering faithful to the page?
    AB,
    /// Is the WebGL tier faithful to the page?
    AC,
    /// Do bunny's two lowerings agree with each other?
    BC,
}

impl Pair {
    pub const ALL: [Self; 3] = [Self::AB, Self::AC, Self::BC];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::AB => "AxB",
            Self::AC => "AxC",
            Self::BC => "BxC",
        }
    }

    fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|pair| pair.name() == name)
    }
}

/// The numbers a pair keeps: nodes whose geometry differs, pixels that
/// differ outside text, pixels that differ under text.
pub type Score = [usize; 3];

pub const COLUMNS: [&str; 3] = ["geometry", "pixels_outside_text", "pixels_in_text"];

/// The table: what it was measured with, and a score per case and pair.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Baseline {
    /// `(key, value)` lines of the header: the browser, the viewport.
    pub about: Vec<(String, String)>,
    pub scores: BTreeMap<(String, Pair), Score>,
}

impl Baseline {
    /// Reads the table.
    ///
    /// # Errors
    ///
    /// A line that is not five tab-separated columns, an unknown pair,
    /// or a count that is not a number, by line.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut baseline = Self::default();
        for (number, line) in text.lines().enumerate() {
            if let Some(about) = line.strip_prefix("# ") {
                if let Some((key, value)) = about.split_once('\t') {
                    baseline.about.push((key.to_owned(), value.to_owned()));
                }
                continue;
            }
            if line.trim().is_empty() || line.starts_with('#') || line.starts_with("case\t") {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let [case, pair, a, b, c] = fields[..] else {
                return Err(format!("line {}: five columns, separated by tabs", number + 1));
            };
            let pair = Pair::named(pair).ok_or_else(|| format!("line {}: `{pair}` is not AxB, AxC or BxC", number + 1))?;
            let count = |field: &str| field.parse::<usize>().map_err(|_| format!("line {}: `{field}` is not a count", number + 1));
            baseline.scores.insert((case.to_owned(), pair), [count(a)?, count(b)?, count(c)?]);
        }
        Ok(baseline)
    }

    #[must_use]
    pub fn write(&self) -> String {
        let mut out = String::from(
            "# The fidelity ratchet: `bunny fidelity sweep` fails when a number grows,\n\
             # and `bunny fidelity sweep --bless` writes the numbers that shrank.\n",
        );
        for (key, value) in &self.about {
            let _ = writeln!(out, "# {key}\t{value}");
        }
        let _ = writeln!(out, "case\tpair\t{}", COLUMNS.join("\t"));
        for ((case, pair), score) in &self.scores {
            let _ = writeln!(out, "{case}\t{}\t{}\t{}\t{}", pair.name(), score[0], score[1], score[2]);
        }
        out
    }

    #[must_use]
    pub fn about(&self, key: &str) -> Option<&str> {
        self.about.iter().find(|(name, _)| name == key).map(|(_, value)| value.as_str())
    }
}

/// What one measured score did against the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Not in the table yet.
    New,
    Same,
    /// Some number shrank and none grew.
    Better,
    /// Some number grew.
    Worse,
}

#[must_use]
pub fn judge(kept: Option<Score>, measured: Score) -> Verdict {
    let Some(kept) = kept else { return Verdict::New };
    if measured.iter().zip(kept).any(|(now, was)| *now > was) {
        Verdict::Worse
    } else if measured == kept {
        Verdict::Same
    } else {
        Verdict::Better
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_round_trips() {
        let mut baseline = Baseline { about: vec![("chrome".to_owned(), "Chrome/150".to_owned())], ..Baseline::default() };
        baseline.scores.insert(("stack".to_owned(), Pair::AC), [1, 20, 300]);
        baseline.scores.insert(("stack".to_owned(), Pair::AB), [0, 0, 4]);
        let text = baseline.write();
        assert!(text.contains("case\tpair\tgeometry\tpixels_outside_text\tpixels_in_text\n"));
        assert!(text.contains("stack\tAxB\t0\t0\t4\nstack\tAxC\t1\t20\t300\n"), "{text}");
        assert_eq!(Baseline::parse(&text).unwrap(), baseline);
        assert_eq!(baseline.about("chrome"), Some("Chrome/150"));
        assert!(Baseline::parse("a\tAxQ\t1\t2\t3").is_err());
        assert!(Baseline::parse("a\tAxB\t1\t2").is_err());
    }

    /// A number that grows fails the case, even when another shrank.
    #[test]
    fn the_ratchet_turns_one_way() {
        assert_eq!(judge(None, [1, 1, 1]), Verdict::New);
        assert_eq!(judge(Some([1, 2, 3]), [1, 2, 3]), Verdict::Same);
        assert_eq!(judge(Some([1, 2, 3]), [0, 2, 3]), Verdict::Better);
        assert_eq!(judge(Some([1, 2, 3]), [0, 3, 0]), Verdict::Worse);
    }
}
