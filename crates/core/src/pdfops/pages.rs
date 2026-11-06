//! Page selections such as `1-3,5`, shared by every engine.

use crate::error::{Error, Result};

/// Most ranges one selection may list, checked before any range is expanded.
pub const MAX_RANGES: usize = 1_000;

/// An inclusive, 1-based page range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRange {
    pub first: u32,
    pub last: u32,
}

impl PageRange {
    /// The pages of the range, in order.
    pub fn pages(&self) -> impl Iterator<Item = u32> {
        self.first..=self.last
    }

    /// The range as `1-3` or `5`, which qpdf and pdfcpu both accept.
    pub fn label(&self) -> String {
        if self.first == self.last {
            self.first.to_string()
        } else {
            format!("{}-{}", self.first, self.last)
        }
    }
}

/// Parses `1-3,5,7-` against a document of `count` pages. An empty
/// selection means every page. Pages run from 1 to `count`; anything else
/// is an [`Error::InvalidRequest`].
pub fn parse(spec: &str, count: u32) -> Result<Vec<PageRange>> {
    if count == 0 {
        return Err(Error::InvalidRequest("the PDF has no pages".into()));
    }
    if spec.trim().is_empty() {
        return Ok(vec![PageRange {
            first: 1,
            last: count,
        }]);
    }
    let bad = |part: &str| Error::InvalidRequest(format!("invalid page selection '{part}'"));
    let mut ranges = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if ranges.len() >= MAX_RANGES {
            return Err(Error::InvalidRequest(format!(
                "a page selection may list at most {MAX_RANGES} ranges"
            )));
        }
        let (first, last) = match part.split_once('-') {
            Some((a, b)) => {
                let first = if a.trim().is_empty() {
                    1
                } else {
                    a.trim().parse::<u32>().map_err(|_| bad(part))?
                };
                let last = if b.trim().is_empty() {
                    count
                } else {
                    b.trim().parse::<u32>().map_err(|_| bad(part))?
                };
                (first, last)
            }
            None => {
                let n = part.parse::<u32>().map_err(|_| bad(part))?;
                (n, n)
            }
        };
        if first == 0 || last < first || last > count {
            return Err(Error::InvalidRequest(format!(
                "page selection '{part}' is outside 1-{count}"
            )));
        }
        ranges.push(PageRange { first, last });
    }
    if ranges.is_empty() {
        return Err(bad(spec));
    }
    Ok(ranges)
}

/// The selected pages, sorted and without duplicates.
pub fn select(spec: &str, count: u32) -> Result<Vec<u32>> {
    let ranges = parse(spec, count)?;
    Ok(merged(&ranges)
        .into_iter()
        .flat_map(|range| range.first..=range.last)
        .collect())
}

/// The ranges sorted and joined where they overlap, so expanding them can never
/// yield more than the document's pages, however often a selection repeats.
pub fn merged(ranges: &[PageRange]) -> Vec<PageRange> {
    let mut sorted = ranges.to_vec();
    sorted.sort_unstable_by_key(|range| range.first);
    let mut out: Vec<PageRange> = Vec::with_capacity(sorted.len());
    for range in sorted {
        match out.last_mut() {
            Some(last) if range.first <= last.last.saturating_add(1) => {
                last.last = last.last.max(range.last);
            }
            _ => out.push(range),
        }
    }
    out
}

/// How many distinct pages the ranges select.
pub fn distinct(ranges: &[PageRange]) -> u64 {
    merged(ranges)
        .iter()
        .map(|range| u64::from(range.last - range.first) + 1)
        .sum()
}

/// Splits `1..=count` into consecutive ranges of `span` pages.
pub fn intervals(span: u32, count: u32) -> Result<Vec<PageRange>> {
    if span == 0 {
        return Err(Error::InvalidRequest(
            "split span must be at least 1".into(),
        ));
    }
    if count == 0 {
        return Err(Error::InvalidRequest("the PDF has no pages".into()));
    }
    Ok((1..=count)
        .step_by(span as usize)
        .map(|first| PageRange {
            first,
            last: (first + span - 1).min(count),
        })
        .collect())
}

/// Joins ranges back into `1-3,5`.
pub fn join(ranges: &[PageRange]) -> String {
    ranges
        .iter()
        .map(PageRange::label)
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections() {
        let r = parse("1-3, 5,7-", 9).unwrap();
        assert_eq!(join(&r), "1-3,5,7-9");
        assert_eq!(select("3,1-2,2", 5).unwrap(), vec![1, 2, 3]);
        assert_eq!(join(&parse("", 4).unwrap()), "1-4");
        assert_eq!(join(&intervals(2, 5).unwrap()), "1-2,3-4,5");
        for bad in ["0", "4-2", "x", "6", "1-7"] {
            assert!(parse(bad, 5).is_err(), "{bad}");
        }
        assert!(intervals(0, 5).is_err());
    }

    #[test]
    fn a_repeated_selection_expands_to_the_document_and_no_further() {
        // Expanded range by range this would be eight gigabytes.
        let spec = "1-,".repeat(MAX_RANGES);
        let started = std::time::Instant::now();
        let pages = select(&spec, 2_000_000).unwrap();
        assert_eq!(pages.len(), 2_000_000);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(select("5,1-2,2-3,7", 9).unwrap(), vec![1, 2, 3, 5, 7]);
        assert_eq!(distinct(&parse("1-3,2-4,9", 9).unwrap()), 5);
    }

    #[test]
    fn too_many_ranges_are_refused_before_expansion() {
        let spec = "1-,".repeat(MAX_RANGES + 1);
        assert!(matches!(parse(&spec, 3), Err(Error::InvalidRequest(_))));
    }
}
