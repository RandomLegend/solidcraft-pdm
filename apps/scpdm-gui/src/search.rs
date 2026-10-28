//! Fuzzy search over every design in the vault (fzf-style, via nucleo).
//!
//! Each word of the query must match somewhere in a design's text: file name, design name, part
//! number, description, project or folder, so "lid petg" finds the lid whose description
//! mentions PETG. Matches in the file name count extra, and their letters are returned for
//! highlighting.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// The searchable text of one design.
pub struct Haystack<'a> {
    /// Matched without its extension: designs end in `.sc` or `.solidcraft`, whose letters would
    /// match almost any query ("lid" is in "so*lid*craft").
    pub file_name: &'a str,
    pub fields: Vec<&'a str>,
}

pub struct Hit {
    pub score: u32,
    /// Char positions in the file name that matched.
    pub file_name_matches: Vec<usize>,
}

pub struct Search {
    pattern: Pattern,
    matcher: Matcher,
    buf: Vec<char>,
}

impl Search {
    /// `None` for an empty query.
    pub fn new(query: &str) -> Option<Self> {
        if query.trim().is_empty() {
            return None;
        }
        Some(Search {
            pattern: Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart),
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            buf: Vec::new(),
        })
    }

    pub fn score(&mut self, hay: &Haystack) -> Option<Hit> {
        let stem = hay.file_name.rsplit_once('.').map_or(hay.file_name, |(stem, _)| stem);
        let stem = if stem.is_empty() { hay.file_name } else { stem };
        let mut total = 0;
        let mut indices = Vec::new();
        for atom in &self.pattern.atoms {
            // Each word must match within one field (letters picked from several fields would
            // match nearly anything); the file name counts double, the best field wins.
            let in_name = atom.indices(Utf32Str::new(stem, &mut self.buf), &mut self.matcher, &mut indices);
            let mut best = in_name.map_or(0, |s| 2 * u32::from(s));
            for field in &hay.fields {
                if let Some(s) = atom.score(Utf32Str::new(field, &mut self.buf), &mut self.matcher) {
                    best = best.max(u32::from(s));
                }
            }
            if best == 0 && in_name.is_none() {
                return None;
            }
            total += best;
        }
        let mut file_name_matches: Vec<usize> = indices.into_iter().map(|i| i as usize).collect();
        file_name_matches.sort_unstable();
        file_name_matches.dedup();
        Some(Hit { score: total, file_name_matches })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hay<'a>(file_name: &'a str, fields: &[&'a str]) -> Haystack<'a> {
        Haystack { file_name, fields: fields.to_vec() }
    }

    #[test]
    fn matches_fuzzily_across_fields() {
        let mut s = Search::new("lid petg").unwrap();
        let hit = s.score(&hay("pi4-lid.solidcraft", &["enclosure", "Snap-fit lid, orange PETG"])).unwrap();
        assert!(!hit.file_name_matches.is_empty());
        assert!(s.score(&hay("pi4-case.solidcraft", &["enclosure", "Vented base"])).is_none());

        let mut s = Search::new("pcase").unwrap();
        assert!(s.score(&hay("pi4-case.solidcraft", &[])).is_some(), "letters in order, gaps allowed");
    }

    #[test]
    fn file_name_matches_rank_first() {
        let mut s = Search::new("bracket").unwrap();
        let by_name = s.score(&hay("bracket.solidcraft", &["parts"])).unwrap();
        let by_description = s.score(&hay("part-17.solidcraft", &["parts", "wall bracket"])).unwrap();
        assert!(by_name.score > by_description.score);
        assert!(by_description.file_name_matches.is_empty());
    }

    #[test]
    fn the_extension_does_not_match() {
        let mut s = Search::new("lid").unwrap();
        assert!(s.score(&hay("pi4-case.solidcraft", &["enclosure"])).is_none());
        assert!(s.score(&hay("pi4-lid.solidcraft", &[])).is_some());
    }

    #[test]
    fn ignores_case_and_empty_queries() {
        assert!(Search::new("  ").is_none());
        let mut s = Search::new("PI4").unwrap();
        assert!(s.score(&hay("pi4-lid.solidcraft", &[])).is_some());
    }
}
