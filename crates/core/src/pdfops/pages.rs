//! Page selections such as `1-3,5`, shared by every engine.

use crate::error::{Error, Result};

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
    let mut pages: Vec<u32> = parse(spec, count)?
        .iter()
        .flat_map(PageRange::pages)
        .collect();
    pages.sort_unstable();
    pages.dedup();
    Ok(pages)
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
}
