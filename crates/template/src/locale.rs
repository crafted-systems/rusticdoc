//! Formatting by language: separators and grouping for `number`, symbols and
//! their placement for `currency`, month and day names for `date`, and named
//! time zones. The data is CLDR's, in `locale_data.rs`, which
//! `scripts/locale-data.mjs` writes.
//!
//! A language with no data falls back to its parent (`fr-lu` to `fr`); one with
//! none at all keeps the plain formatting, the same as no language.

use chrono::{FixedOffset, NaiveDateTime, TimeZone};

use crate::locale_data::{CURRENCY_DIGITS, LOCALES};

pub use crate::locale_data::CLDR_VERSION;

/// Longest language tag accepted: BCP 47 tags in use are far shorter, and a
/// tag becomes part of lookup keys.
pub const MAX_LANG_LEN: usize = 35;

/// How one language writes numbers and money.
#[derive(Debug)]
pub(crate) struct NumberData {
    pub decimal: &'static str,
    pub group: &'static str,
    /// The separators money is written with, which in a few languages are
    /// not those of plain numbers: Austria writes `1 234,5` but `€ 1.234,50`.
    pub money_decimal: &'static str,
    pub money_group: &'static str,
    /// Digits in the group nearest the decimal separator.
    pub primary: u8,
    /// Digits in every further group: 2 in India, 3 nearly everywhere else.
    pub secondary: u8,
    /// Integer digits a number needs past `primary` before it is grouped at
    /// all: 2 in Spanish and Polish, which write `1234` but `12 345`.
    pub min_grouping: u8,
    /// Positive and negative patterns, `#` standing for the digits. The
    /// negative one holds the language's minus sign and any direction marks.
    pub number: [&'static str; 2],
    /// Positive and negative money patterns, `¤` standing for the symbol,
    /// for symbols such as `€` and `US$`.
    pub symbol: [&'static str; 2],
    /// The same for a symbol that is letters next to the digits, such as
    /// `CHF`, which CLDR keeps apart from them with a no-break space.
    pub letters: [&'static str; 2],
    /// `(code, symbol)` for each currency whose symbol is not its code,
    /// sorted by code.
    pub symbols: &'static [(&'static str, &'static str)],
}

/// Month and day names in one language.
#[derive(Debug)]
pub(crate) struct DateNames {
    /// January first: wide and abbreviated names as they read in a date
    /// ("5 января"), then as they read alone ("январь").
    pub months: [[&'static str; 12]; 4],
    /// Monday first: wide, then abbreviated.
    pub weekdays: [[&'static str; 7]; 2],
    pub am_pm: [&'static str; 2],
}

/// One language's data.
#[derive(Debug)]
pub(crate) struct LocaleData {
    pub tag: &'static str,
    pub number: NumberData,
    pub dates: &'static DateNames,
}

/// The formatting for no language, or one without data: `,` between
/// thousands and `.` before decimals, as `currency` has always printed.
pub(crate) static PLAIN: NumberData = NumberData {
    decimal: ".",
    group: ",",
    money_decimal: ".",
    money_group: ",",
    primary: 3,
    secondary: 3,
    min_grouping: 1,
    number: ["#", "-#"],
    symbol: ["¤#", "-¤#"],
    letters: ["¤\u{a0}#", "-¤\u{a0}#"],
    symbols: &[],
};

/// `fr-FR`, `fr_fr` and ` FR-fr ` all become `fr-fr`. None for anything that
/// is not shaped like a BCP 47 tag: two or three letters, then subtags of one
/// to eight letters or digits.
pub fn normalize_lang(raw: &str) -> Option<String> {
    let tag = raw.trim().to_ascii_lowercase().replace('_', "-");
    if tag.is_empty() || tag.len() > MAX_LANG_LEN {
        return None;
    }
    let mut parts = tag.split('-');
    let first = parts.next()?;
    if !(2..=3).contains(&first.len()) || !first.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    for part in parts {
        if !(1..=8).contains(&part.len()) || !part.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return None;
        }
    }
    Some(tag)
}

/// The tag, then each shorter tag it falls back to: `zh-hant-tw`, `zh-hant`,
/// `zh`. The tag should be normalised.
pub fn fallbacks(lang: &str) -> impl Iterator<Item = &str> {
    let mut next = Some(lang);
    std::iter::from_fn(move || {
        let current = next?;
        next = current.rfind('-').map(|i| &current[..i]);
        Some(current)
    })
}

/// The data for a language, or for the nearest tag it falls back to.
pub(crate) fn find(lang: &str) -> Option<&'static LocaleData> {
    let lang = normalize_lang(lang)?;
    let found = fallbacks(&lang).find_map(|tag| {
        LOCALES
            .binary_search_by(|l| l.tag.cmp(tag))
            .ok()
            .map(|i| &LOCALES[i])
    });
    found
}

/// True when there is formatting data for this language or a tag it falls
/// back to.
pub fn has_data(lang: &str) -> bool {
    find(lang).is_some()
}

/// The decimals a currency is written with: 0 for JPY, 3 for KWD, 2 for most
/// and for codes CLDR does not know.
pub fn currency_digits(code: &str) -> usize {
    let code = code.trim().to_ascii_uppercase();
    CURRENCY_DIGITS
        .binary_search_by(|(c, _)| (*c).cmp(code.as_str()))
        .map(|i| CURRENCY_DIGITS[i].1 as usize)
        .unwrap_or(2)
}

/// How many decimals `number` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decimals {
    /// Up to three, with trailing zeros dropped, as CLDR's default pattern.
    Upto3,
    /// Exactly this many.
    Exactly(usize),
}

/// `value` with the separators of `data`.
pub(crate) fn format_number(data: &NumberData, value: f64, decimals: Decimals) -> String {
    let (digits, negative) = digits(data, value, decimals, false);
    data.number[usize::from(negative)].replace('#', &digits)
}

/// `value` as money in `code`, the way `data`'s language writes it: the
/// symbol where the language puts it, and the currency's own decimals. An
/// empty code prints the amount alone.
pub(crate) fn format_currency(data: &NumberData, value: f64, code: &str) -> String {
    let code = code.trim();
    let places = currency_digits(code);
    let (digits, negative) = digits(data, value, Decimals::Exactly(places), true);
    if code.is_empty() {
        return data.number[usize::from(negative)].replace('#', &digits);
    }
    let upper = code.to_ascii_uppercase();
    let symbol = data
        .symbols
        .binary_search_by(|(c, _)| (*c).cmp(upper.as_str()))
        .map(|i| data.symbols[i].1)
        .unwrap_or(code);
    // CLDR's currency spacing: a symbol whose letter touches the digits is
    // kept apart from them.
    let before = data.letters[0].find('¤') < data.letters[0].find('#');
    let touching = if before {
        symbol.chars().last()
    } else {
        symbol.chars().next()
    };
    let patterns = if touching.is_some_and(char::is_alphabetic) {
        &data.letters
    } else {
        &data.symbol
    };
    patterns[usize::from(negative)]
        .replace('#', &digits)
        .replace('¤', symbol)
}

/// The digits of `|value|` with separators, and whether to write it as
/// negative. Rounding is half away from zero on the shortest decimal form of
/// the number, as CLDR formatters do, so `1.005` rounds to `1.01`. `money`
/// picks the language's separators for money.
fn digits(data: &NumberData, value: f64, decimals: Decimals, money: bool) -> (String, bool) {
    let (decimal, separator) = if money {
        (data.money_decimal, data.money_group)
    } else {
        (data.decimal, data.group)
    };
    let negative = value < 0.0 || (value == 0.0 && value.is_sign_negative());
    if value.is_nan() {
        return ("NaN".into(), false);
    }
    if value.is_infinite() {
        return ("∞".into(), negative);
    }
    let shortest = format!("{}", value.abs());
    let (int, frac) = shortest.split_once('.').unwrap_or((&shortest, ""));
    let (min, max) = match decimals {
        Decimals::Upto3 => (0, 3),
        Decimals::Exactly(n) => (n, n),
    };
    let mut all: Vec<u8> = int.bytes().chain(frac.bytes()).collect();
    let mut point = int.len();
    if frac.len() > max {
        let round_up = frac.as_bytes()[max] >= b'5';
        all.truncate(point + max);
        if round_up {
            let mut i = all.len();
            loop {
                if i == 0 {
                    all.insert(0, b'1');
                    point += 1;
                    break;
                }
                i -= 1;
                if all[i] == b'9' {
                    all[i] = b'0';
                } else {
                    all[i] += 1;
                    break;
                }
            }
        }
    }
    let mut frac: Vec<u8> = all.split_off(point);
    while frac.len() > min && frac.last() == Some(&b'0') {
        frac.pop();
    }
    while frac.len() < min {
        frac.push(b'0');
    }
    let int = String::from_utf8(all).unwrap_or_default();
    let int = if int.is_empty() { "0".to_string() } else { int };
    let mut out = group(data, &int, separator);
    if !frac.is_empty() {
        out.push_str(decimal);
        out.push_str(std::str::from_utf8(&frac).unwrap_or_default());
    }
    (out, negative)
}

/// Integer digits with group separators.
fn group(data: &NumberData, int: &str, separator: &str) -> String {
    let primary = data.primary as usize;
    let secondary = data.secondary as usize;
    if primary == 0 || int.len() < primary + data.min_grouping as usize {
        return int.to_string();
    }
    let mut groups: Vec<&str> = Vec::new();
    let mut end = int.len();
    let mut size = primary;
    while end > size {
        groups.push(&int[end - size..end]);
        end -= size;
        size = secondary.max(1);
    }
    groups.push(&int[..end]);
    groups.reverse();
    groups.join(separator)
}

/// A time zone: a fixed offset, or an IANA zone that follows daylight saving.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Zone {
    Fixed(FixedOffset),
    Named(chrono_tz::Tz),
}

impl Zone {
    /// `+02:00`, `-0530`, `UTC`, `Z`, `GMT`, or a zone such as `Europe/Paris`.
    /// An offset is read as it always has been: `+2` is two hours.
    pub(crate) fn parse(tz: &str) -> Result<Zone, String> {
        let tz = tz.trim();
        if tz.is_empty()
            || tz.eq_ignore_ascii_case("utc")
            || tz == "Z"
            || tz.eq_ignore_ascii_case("gmt")
        {
            return Ok(Zone::Fixed(FixedOffset::east_opt(0).expect("zero offset")));
        }
        let (sign, rest) = match tz.chars().next() {
            Some('+') => (1, &tz[1..]),
            Some('-') => (-1, &tz[1..]),
            _ => {
                return tz.parse::<chrono_tz::Tz>().map(Zone::Named).map_err(|_| {
                    format!(
                        "timezone {tz:?} is neither an offset like +02:00 nor an IANA zone like Europe/Paris"
                    )
                });
            }
        };
        let (h, m) = rest.split_once(':').unwrap_or((rest, "0"));
        let secs = h.parse::<i32>().unwrap_or(0) * 3600 + m.parse::<i32>().unwrap_or(0) * 60;
        FixedOffset::east_opt(sign * secs)
            .map(Zone::Fixed)
            .ok_or_else(|| format!("bad timezone {tz:?}"))
    }

    /// The local time of a moment, here.
    pub(crate) fn local<Tz: TimeZone>(&self, moment: &chrono::DateTime<Tz>) -> NaiveDateTime {
        match self {
            Zone::Fixed(offset) => moment.with_timezone(offset).naive_local(),
            Zone::Named(zone) => moment.with_timezone(zone).naive_local(),
        }
    }
}

/// Checks a `timezone` value before any render, for callers that want to
/// refuse a bad one up front.
pub fn check_timezone(tz: &str) -> Result<(), String> {
    Zone::parse(tz).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_normalise() {
        assert_eq!(normalize_lang(" FR_ca ").as_deref(), Some("fr-ca"));
        assert_eq!(normalize_lang("en-US").as_deref(), Some("en-us"));
        assert_eq!(normalize_lang("zh-Hant-TW").as_deref(), Some("zh-hant-tw"));
        assert_eq!(normalize_lang("es-419").as_deref(), Some("es-419"));
        for bad in [
            "",
            "f",
            "french",
            "fr--ca",
            "fr-",
            "1a",
            "fr ca",
            "fr-abcdefghi",
        ] {
            assert_eq!(normalize_lang(bad), None, "{bad:?}");
        }
        let chain: Vec<&str> = fallbacks("zh-hant-tw").collect();
        assert_eq!(chain, ["zh-hant-tw", "zh-hant", "zh"]);
    }

    #[test]
    fn data_is_sorted_and_found_through_fallbacks() {
        assert!(LOCALES.windows(2).all(|w| w[0].tag < w[1].tag));
        assert!(CURRENCY_DIGITS.windows(2).all(|w| w[0].0 < w[1].0));
        for l in LOCALES {
            assert!(
                l.number.symbols.windows(2).all(|w| w[0].0 < w[1].0),
                "{}",
                l.tag
            );
        }
        assert_eq!(find("fr-LU").map(|l| l.tag), Some("fr"));
        assert_eq!(find("en-us").map(|l| l.tag), Some("en"));
        assert_eq!(find("de-CH").map(|l| l.tag), Some("de-ch"));
        assert!(find("xx").is_none());
        assert_eq!(currency_digits("JPY"), 0);
        assert_eq!(currency_digits("kwd"), 3);
        assert_eq!(currency_digits("EUR"), 2);
        assert_eq!(currency_digits("ZZZ"), 2);
    }

    #[test]
    fn rounding_works_on_the_decimal_form() {
        let en = &find("en").unwrap().number;
        assert_eq!(format_number(en, 1.005, Decimals::Exactly(2)), "1.01");
        assert_eq!(format_number(en, 999.9995, Decimals::Upto3), "1,000");
        assert_eq!(format_number(en, 9.5, Decimals::Exactly(0)), "10");
        assert_eq!(format_number(en, -0.0001, Decimals::Exactly(2)), "-0.00");
        assert_eq!(
            format_number(en, 1e21, Decimals::Upto3),
            "1,000,000,000,000,000,000,000"
        );
        assert_eq!(format_number(en, 0.1 + 0.2, Decimals::Upto3), "0.3");
    }

    #[test]
    fn zones_parse() {
        assert!(matches!(Zone::parse("+02:00"), Ok(Zone::Fixed(_))));
        assert!(matches!(Zone::parse("Europe/Paris"), Ok(Zone::Named(_))));
        assert!(check_timezone("America/New_York").is_ok());
        let err = check_timezone("Mars/Olympus").unwrap_err();
        assert!(err.contains("IANA zone"), "{err}");
    }
}
