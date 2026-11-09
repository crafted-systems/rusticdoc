//! Calculation and condition helpers that every template kind gets: Office,
//! text and PDF form tags.
//!
//! - the `empty` test;
//! - text filters: `truncate`, `unaccent`, `pad_left`, `pad_right`;
//! - number filters: `avg`, `ceil`, `floor`, `words`;
//! - date arithmetic: `date_add`, `date_sub`, `start_of`, `end_of`,
//!   `date_diff`, and the `now()` function;
//! - `duration`, for an interval in milliseconds or ISO 8601.
//!
//! Dates are read the way the `date` filter reads them and are returned as
//! ISO 8601 strings, so `date` can print the result. All arithmetic is on
//! fixed offsets (UTC unless the value names one), so there is no daylight
//! saving to trip over.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{
    DateTime, Datelike, FixedOffset, Months, NaiveDate, NaiveDateTime, NaiveTime, SecondsFormat,
    TimeDelta, TimeZone, Timelike, Utc,
};
use minijinja::value::{Kwargs, Value, ValueKind};
use minijinja::{Environment, Error, ErrorKind, State, UndefinedBehavior};
use unicode_normalization::UnicodeNormalization;

/// The widest `pad_left`/`pad_right` may make a value, in characters.
pub const MAX_WIDTH: usize = 10_000;

/// `words` spells numbers below this magnitude: up to 999 trillion.
pub const MAX_WORDS: f64 = 1e15;

/// Adds the helpers to an environment. `lang` is the template's `lang`
/// setting, the default language of `words`.
pub fn add_filters(env: &mut Environment<'_>, lang: Option<&str>) {
    env.add_test("empty", is_empty);
    env.add_filter("truncate", truncate);
    env.add_filter("unaccent", unaccent);
    env.add_filter(
        "pad_left",
        |v: &Value, w: i64, c: Option<String>, kw: Kwargs| pad(v, w, c, kw, true),
    );
    env.add_filter(
        "pad_right",
        |v: &Value, w: i64, c: Option<String>, kw: Kwargs| pad(v, w, c, kw, false),
    );
    env.add_filter("avg", avg);
    env.add_filter("ceil", |v: &Value| round_with(v, "ceil", f64::ceil));
    env.add_filter("floor", |v: &Value| round_with(v, "floor", f64::floor));
    let lang = lang.map(str::to_string);
    env.add_filter("words", move |v: &Value, kw: Kwargs| {
        words(v, kw, lang.as_deref())
    });
    env.add_filter("date_add", |v: &Value, n: i64, unit: Option<String>| {
        date_add("date_add", v, n, unit.as_deref())
    });
    env.add_filter("date_sub", |v: &Value, n: i64, unit: Option<String>| {
        let n = n.checked_neg().ok_or_else(|| out_of_range("date_sub"))?;
        date_add("date_sub", v, n, unit.as_deref())
    });
    env.add_filter("start_of", |v: &Value, unit: String| {
        boundary("start_of", v, &unit, false)
    });
    env.add_filter("end_of", |v: &Value, unit: String| {
        boundary("end_of", v, &unit, true)
    });
    env.add_filter(
        "date_diff",
        |v: &Value, other: &Value, unit: Option<String>| date_diff(v, other, unit.as_deref()),
    );
    env.add_filter("duration", |v: &Value, unit: Option<String>| {
        duration(v, unit.as_deref())
    });
    env.add_function("now", now);
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidOperation, msg.into())
}

// ---------------------------------------------------------------- tests

/// `x is empty`: true for undefined, `null`, `""`, `[]` and `{}`. A number or
/// a boolean is never empty, so `0` and `false` are not.
pub fn is_empty(value: &Value) -> bool {
    match value.kind() {
        ValueKind::Undefined | ValueKind::None => true,
        ValueKind::String | ValueKind::Seq | ValueKind::Map | ValueKind::Bytes => {
            value.len() == Some(0)
        }
        ValueKind::Iterable => value
            .try_iter()
            .map(|mut i| i.next().is_none())
            .unwrap_or(false),
        _ => false,
    }
}

// ---------------------------------------------------------------- text

/// A value as text for the text filters: strings as they are, numbers and
/// booleans written out, `null` as nothing. Lists and objects are refused.
fn text(name: &str, value: &Value) -> Result<String, Error> {
    match value.kind() {
        ValueKind::None | ValueKind::Undefined => Ok(String::new()),
        ValueKind::String | ValueKind::Number | ValueKind::Bool => Ok(match value.as_str() {
            Some(s) => s.to_string(),
            None => value.to_string(),
        }),
        kind => Err(invalid(format!(
            "{name}: expected text or a number, got {kind}"
        ))),
    }
}

/// `truncate(length, end="…", words=false)`: at most `length` characters,
/// `end` included. With `words=true` the cut falls between words.
pub fn truncate(
    value: &Value,
    length: i64,
    end: Option<String>,
    kwargs: Kwargs,
) -> Result<Value, Error> {
    let end = match end {
        Some(end) => end,
        None => kwargs
            .get::<Option<String>>("end")?
            .unwrap_or_else(|| "…".to_string()),
    };
    let whole_words = kwargs.get::<Option<bool>>("words")?.unwrap_or(false);
    kwargs.assert_all_used()?;
    let length = usize::try_from(length).map_err(|_| {
        invalid(format!(
            "truncate: the length must be 0 or more, got {length}"
        ))
    })?;
    let s = text("truncate", value)?;
    Ok(Value::from(truncate_str(&s, length, &end, whole_words)))
}

fn truncate_str(s: &str, length: usize, end: &str, whole_words: bool) -> String {
    if s.chars().count() <= length {
        return s.to_string();
    }
    let end_len = end.chars().count();
    if end_len >= length {
        return end.chars().take(length).collect();
    }
    let keep = length - end_len;
    // Byte index just past the `keep`th character.
    let cut = s.char_indices().nth(keep).map_or(s.len(), |(i, _)| i);
    let mut head = &s[..cut];
    if whole_words {
        let next_is_break = s[cut..].chars().next().is_none_or(char::is_whitespace);
        if !next_is_break {
            head = match head.rfind(char::is_whitespace) {
                Some(i) => &head[..i],
                None => "",
            };
        }
        head = head.trim_end();
    }
    format!("{head}{end}")
}

/// `unaccent`: the text with accents and other diacritics removed, so
/// `crème brûlée` gives `creme brulee`. Letters that are not a base letter
/// plus a mark, such as `ß` or `ø`, are spelled with plain letters.
pub fn unaccent(value: &Value) -> Result<Value, Error> {
    let s = text("unaccent", value)?;
    let mut out = String::with_capacity(s.len());
    for c in s.nfd() {
        if is_diacritic(c) {
            continue;
        }
        match c {
            'ß' => out.push_str("ss"),
            'ẞ' => out.push_str("SS"),
            'æ' => out.push_str("ae"),
            'Æ' => out.push_str("AE"),
            'œ' => out.push_str("oe"),
            'Œ' => out.push_str("OE"),
            'ø' => out.push('o'),
            'Ø' => out.push('O'),
            'đ' | 'ð' => out.push('d'),
            'Đ' | 'Ð' => out.push('D'),
            'ł' => out.push('l'),
            'Ł' => out.push('L'),
            'ħ' => out.push('h'),
            'Ħ' => out.push('H'),
            'ı' => out.push('i'),
            'þ' => out.push_str("th"),
            'Þ' => out.push_str("Th"),
            c => out.push(c),
        }
    }
    // Recompose what decomposition split but did not strip, such as Hangul.
    Ok(Value::from(out.nfc().collect::<String>()))
}

/// Combining marks that are accents. Vowel signs of Indic scripts and the
/// Japanese voicing marks are also combining marks, but removing them would
/// change the letter, so they stay.
fn is_diacritic(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036F}'
        | '\u{1AB0}'..='\u{1AFF}'
        | '\u{1DC0}'..='\u{1DFF}'
        | '\u{20D0}'..='\u{20FF}'
        | '\u{FE20}'..='\u{FE2F}')
}

/// `pad_left(width, char=" ")` and `pad_right`: the text filled out to
/// `width` characters. Longer text is left as it is.
fn pad(
    value: &Value,
    width: i64,
    fill: Option<String>,
    kwargs: Kwargs,
    left: bool,
) -> Result<Value, Error> {
    let name = if left { "pad_left" } else { "pad_right" };
    let fill = match fill {
        Some(fill) => fill,
        None => kwargs
            .get::<Option<String>>("char")?
            .unwrap_or_else(|| " ".to_string()),
    };
    kwargs.assert_all_used()?;
    let width = usize::try_from(width)
        .ok()
        .filter(|w| *w <= MAX_WIDTH)
        .ok_or_else(|| {
            invalid(format!(
                "{name}: the width must be between 0 and {MAX_WIDTH}, got {width}"
            ))
        })?;
    let mut chars = fill.chars();
    let (Some(fill), None) = (chars.next(), chars.next()) else {
        return Err(invalid(format!(
            "{name}: the fill must be one character, got {fill:?}"
        )));
    };
    let s = text(name, value)?;
    let missing = width.saturating_sub(s.chars().count());
    let padding: String = std::iter::repeat_n(fill, missing).collect();
    Ok(Value::from(if left { padding + &s } else { s + &padding }))
}

// ---------------------------------------------------------------- numbers

/// Follows a dotted path such as `price` or `line.0.price` into an item;
/// undefined when any step is missing.
fn path(item: &Value, attribute: &str) -> Value {
    let mut current = item.clone();
    for part in attribute.split('.') {
        let next = match part.parse::<usize>() {
            Ok(i) => current.get_item_by_index(i),
            Err(_) => current.get_attr(part),
        };
        match next {
            Ok(v) if !v.is_undefined() => current = v,
            _ => return Value::UNDEFINED,
        }
    }
    current
}

/// `avg` or `avg(attribute="price")`: the mean of a list. Items whose value
/// is missing or `null` are left out; an empty list gives `null`.
pub fn avg(
    state: &State,
    values: &Value,
    attribute: Option<String>,
    kwargs: Kwargs,
) -> Result<Value, Error> {
    let attribute = match attribute {
        Some(a) => Some(a),
        None => kwargs.get::<Option<String>>("attribute")?,
    };
    kwargs.assert_all_used()?;
    if values.is_undefined() && state.undefined_behavior() == UndefinedBehavior::Strict {
        return Err(Error::from(ErrorKind::UndefinedError));
    }
    let iter = values.try_iter()?;
    let mut total = 0.0;
    let mut count = 0usize;
    for item in iter {
        let value = match &attribute {
            Some(a) => path(&item, a),
            None => item,
        };
        if value.is_undefined() || value.is_none() {
            continue;
        }
        if !value.is_number() {
            return Err(invalid(format!(
                "avg: can only average numbers, got {}",
                value.kind()
            )));
        }
        total += f64::try_from(value)?;
        count += 1;
    }
    if count == 0 {
        return Ok(Value::from(()));
    }
    Ok(number(total / count as f64))
}

/// A float as an integer when it is a whole number that fits, so `2.0`
/// prints as `2`.
fn number(x: f64) -> Value {
    if x.fract() == 0.0 && x.abs() < 9.0e15 {
        Value::from(x as i64)
    } else {
        Value::from(x)
    }
}

/// `ceil` and `floor`: the next whole number up or down, as an integer.
fn round_with(value: &Value, name: &str, f: fn(f64) -> f64) -> Result<Value, Error> {
    if value.is_integer() {
        return Ok(value.clone());
    }
    if !value.is_number() {
        return Err(invalid(format!(
            "{name}: expected a number, got {}",
            value.kind()
        )));
    }
    let x = f(f64::try_from(value.clone())?);
    if x.is_finite() && x.abs() < 9.2e18 {
        Ok(Value::from(x as i64))
    } else {
        Err(invalid(format!("{name}: {x} is out of range")))
    }
}

/// `words`: a number spelled out. `ordinal=true` gives `forty-second`,
/// `currency="EUR"` gives `forty-two euros`, and `lang=` overrides the
/// template's language.
pub fn words(value: &Value, kwargs: Kwargs, default_lang: Option<&str>) -> Result<Value, Error> {
    use num2words::{Currency, Num2Words};
    let ordinal = kwargs.get::<Option<bool>>("ordinal")?.unwrap_or(false);
    let currency = kwargs.get::<Option<String>>("currency")?;
    let explicit = kwargs.get::<Option<String>>("lang")?;
    kwargs.assert_all_used()?;
    if !value.is_number() {
        return Err(invalid(format!(
            "words: expected a number, got {}",
            value.kind()
        )));
    }
    let x = f64::try_from(value.clone())?;
    if !x.is_finite() || x.abs() >= MAX_WORDS {
        return Err(invalid(format!(
            "words: {x} is too large; the limit is 999,999,999,999,999"
        )));
    }
    let lang = match explicit.as_deref() {
        Some(l) => words_lang(l).ok_or_else(|| {
            invalid(format!(
                "words: no words for language {l:?}; use en, fr or uk"
            ))
        })?,
        None => default_lang
            .and_then(words_lang)
            .unwrap_or(num2words::Lang::English),
    };
    let english = matches!(lang, num2words::Lang::English);
    // Through text, so 12.29 is the decimal 12.29 and not 12.2899...
    let digits = match (&currency, value.is_integer()) {
        (Some(_), _) => format!("{x:.2}"),
        (None, true) => value.to_string(),
        (None, false) => x.to_string(),
    };
    let mut builder = Num2Words::parse(&digits)
        .ok_or_else(|| invalid(format!("words: cannot read {digits}")))?
        .lang(lang);
    match (ordinal, currency) {
        (true, Some(_)) => {
            return Err(invalid("words: use ordinal or currency, not both"));
        }
        (true, None) => builder = builder.ordinal(),
        (false, Some(code)) => {
            let c = code.trim().to_ascii_uppercase().parse::<Currency>().map_err(|_| {
                invalid(format!(
                    "words: no words for currency {code:?}; use an ISO 4217 code such as EUR, USD or GBP"
                ))
            })?;
            builder = builder.currency(c);
        }
        (false, None) => {}
    }
    let spelled = builder
        .to_words()
        .map_err(|e| invalid(format!("words: {e}")))?;
    if !english {
        return Ok(Value::from(spelled));
    }
    // num2words writes the British "two thousand and five"; write it without
    // that "and", keeping the one before the cents.
    static AND: OnceLock<regex::Regex> = OnceLock::new();
    let and = AND.get_or_init(|| {
        regex::Regex::new(r"\b(hundred|thousand|million|billion|trillion) and ")
            .expect("and pattern")
    });
    Ok(Value::from(and.replace_all(&spelled, "$1 ").into_owned()))
}

/// The `num2words` language for a tag such as `fr`, `fr-BE` or `en_GB`.
fn words_lang(tag: &str) -> Option<num2words::Lang> {
    use num2words::Lang;
    let tag = tag.trim().replace('-', "_");
    let (base, region) = tag.split_once('_').unwrap_or((&tag, ""));
    Some(
        match (
            base.to_ascii_lowercase().as_str(),
            region.to_ascii_uppercase().as_str(),
        ) {
            ("en", _) => Lang::English,
            ("fr", "BE") => Lang::French_BE,
            ("fr", "CH") => Lang::French_CH,
            ("fr", _) => Lang::French,
            ("uk", _) => Lang::Ukrainian,
            _ => return None,
        },
    )
}

// ---------------------------------------------------------------- dates

/// A point in time as a template wrote it. The kind is kept so a date stays
/// a date after adding days to it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum When {
    /// `2026-11-06`.
    Date(NaiveDate),
    /// `2026-11-06T10:20:30`, with no zone.
    Local(NaiveDateTime),
    /// `2026-11-06T10:20:30+02:00`, or unix seconds (in UTC).
    Zoned(DateTime<FixedOffset>),
}

fn out_of_range(name: &str) -> Error {
    invalid(format!("{name}: the date is out of range"))
}

/// Reads a value the way the `date` filter does.
fn parse_when(name: &str, value: &Value) -> Result<When, Error> {
    if let Some(s) = value.as_str() {
        let s = s.trim();
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Ok(When::Zoned(dt));
        }
        if let Ok(dt) = DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%z") {
            return Ok(When::Zoned(dt));
        }
        for fmt in [
            "%Y-%m-%dT%H:%M:%S%.f",
            "%Y-%m-%d %H:%M:%S%.f",
            "%Y-%m-%dT%H:%M",
        ] {
            if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
                return Ok(When::Local(dt));
            }
        }
        if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            return Ok(When::Date(d));
        }
        return Err(invalid(format!(
            "{name}: cannot parse {s:?}; use ISO 8601 or unix seconds"
        )));
    }
    if value.is_number() {
        let secs = f64::try_from(value.clone())?;
        let whole = secs.floor();
        if !whole.is_finite() || whole.abs() > 1e16 {
            return Err(out_of_range(name));
        }
        let nanos = ((secs - whole) * 1e9) as u32;
        return Utc
            .timestamp_opt(whole as i64, nanos)
            .single()
            .map(|dt| When::Zoned(dt.fixed_offset()))
            .ok_or_else(|| out_of_range(name));
    }
    Err(invalid(format!(
        "{name}: expected a date string or unix seconds, got {}",
        value.kind()
    )))
}

impl When {
    /// The wall-clock time, in the value's own offset.
    fn naive(self) -> NaiveDateTime {
        match self {
            When::Date(d) => d.and_time(NaiveTime::MIN),
            When::Local(dt) => dt,
            When::Zoned(dt) => dt.naive_local(),
        }
    }

    /// The moment, for comparing two values. A value with no zone counts as
    /// UTC.
    fn utc(self) -> NaiveDateTime {
        match self {
            When::Zoned(dt) => dt.naive_utc(),
            other => other.naive(),
        }
    }

    /// The same kind of value at another wall-clock time. A date that gains
    /// a time of day becomes a local time.
    fn with(self, name: &str, naive: NaiveDateTime, keep_date: bool) -> Result<When, Error> {
        Ok(match self {
            When::Date(_) if keep_date => When::Date(naive.date()),
            When::Date(_) | When::Local(_) => When::Local(naive),
            When::Zoned(dt) => When::Zoned(
                dt.offset()
                    .from_local_datetime(&naive)
                    .single()
                    .ok_or_else(|| out_of_range(name))?,
            ),
        })
    }

    fn into_value(self) -> Value {
        Value::from(match self {
            When::Date(d) => d.format("%Y-%m-%d").to_string(),
            When::Local(dt) => dt.format("%Y-%m-%dT%H:%M:%S%.f").to_string(),
            When::Zoned(dt) => dt.to_rfc3339_opts(SecondsFormat::AutoSi, true),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Year,
    Quarter,
    Month,
    Week,
    Day,
    Hour,
    Minute,
    Second,
}

impl Unit {
    fn parse(name: &str, unit: Option<&str>) -> Result<Unit, Error> {
        let raw = unit.unwrap_or("day").trim().to_ascii_lowercase();
        let singular = raw.strip_suffix('s').unwrap_or(&raw);
        Ok(match singular {
            "year" => Unit::Year,
            "quarter" => Unit::Quarter,
            "month" => Unit::Month,
            "week" => Unit::Week,
            "day" => Unit::Day,
            "hour" => Unit::Hour,
            "minute" => Unit::Minute,
            "second" => Unit::Second,
            _ => {
                return Err(invalid(format!(
                    "{name}: unknown unit {raw:?}; use year, quarter, month, week, day, hour, minute or second"
                )))
            }
        })
    }

    /// Whether a date with no time stays a date in this unit.
    fn keeps_date(self) -> bool {
        !matches!(self, Unit::Hour | Unit::Minute | Unit::Second)
    }

    /// Months in one unit, for the calendar units.
    fn months(self) -> Option<i64> {
        match self {
            Unit::Year => Some(12),
            Unit::Quarter => Some(3),
            Unit::Month => Some(1),
            _ => None,
        }
    }

    /// Seconds in one unit, for the fixed-length units.
    fn seconds(self) -> i64 {
        match self {
            Unit::Week => 7 * 86_400,
            Unit::Day => 86_400,
            Unit::Hour => 3_600,
            Unit::Minute => 60,
            _ => 1,
        }
    }
}

/// Adds `n` months, keeping the day when the month has it and using the
/// month's last day when it does not.
fn add_months(t: NaiveDateTime, n: i64) -> Option<NaiveDateTime> {
    let months = Months::new(u32::try_from(n.unsigned_abs()).ok()?);
    if n >= 0 {
        t.checked_add_months(months)
    } else {
        t.checked_sub_months(months)
    }
}

fn shift(t: NaiveDateTime, n: i64, unit: Unit) -> Option<NaiveDateTime> {
    match unit.months() {
        Some(per) => add_months(t, n.checked_mul(per)?),
        None => t.checked_add_signed(TimeDelta::try_seconds(n.checked_mul(unit.seconds())?)?),
    }
}

/// `date_add(amount, unit)` and `date_sub`: the date moved by whole units.
fn date_add(name: &str, value: &Value, n: i64, unit: Option<&str>) -> Result<Value, Error> {
    let when = parse_when(name, value)?;
    let unit = Unit::parse(name, unit)?;
    let moved = shift(when.naive(), n, unit).ok_or_else(|| out_of_range(name))?;
    Ok(when.with(name, moved, unit.keeps_date())?.into_value())
}

/// `start_of(unit)` and `end_of(unit)`: the first or last second of the
/// year, quarter, month, week (Monday to Sunday), day, hour or minute.
fn boundary(name: &str, value: &Value, unit: &str, end: bool) -> Result<Value, Error> {
    let when = parse_when(name, value)?;
    let unit = Unit::parse(name, Some(unit))?;
    let t = when.naive();
    let date = t.date();
    let start = match unit {
        Unit::Year => {
            NaiveDate::from_ymd_opt(date.year(), 1, 1).map(|d| d.and_time(NaiveTime::MIN))
        }
        Unit::Quarter => NaiveDate::from_ymd_opt(date.year(), (date.month0() / 3) * 3 + 1, 1)
            .map(|d| d.and_time(NaiveTime::MIN)),
        Unit::Month => date.with_day(1).map(|d| d.and_time(NaiveTime::MIN)),
        Unit::Week => date
            .checked_sub_signed(TimeDelta::days(i64::from(
                date.weekday().num_days_from_monday(),
            )))
            .map(|d| d.and_time(NaiveTime::MIN)),
        Unit::Day => Some(date.and_time(NaiveTime::MIN)),
        Unit::Hour => t.with_minute(0).and_then(|t| t.with_second(0)),
        Unit::Minute => t.with_second(0),
        Unit::Second => Some(t),
    }
    .and_then(|s| s.with_nanosecond(0))
    .ok_or_else(|| out_of_range(name))?;
    let result = if end {
        shift(start, 1, unit)
            .and_then(|next| next.checked_sub_signed(TimeDelta::seconds(1)))
            .ok_or_else(|| out_of_range(name))?
    } else {
        start
    };
    Ok(when.with(name, result, unit.keeps_date())?.into_value())
}

/// `a | date_diff(b, unit)`: whole units from `b` to `a`, negative when `a`
/// is earlier. Partial units are dropped, so the count is toward zero.
fn date_diff(value: &Value, other: &Value, unit: Option<&str>) -> Result<Value, Error> {
    let name = "date_diff";
    let a = parse_when(name, value)?.utc();
    let b = parse_when(name, other)?.utc();
    let unit = Unit::parse(name, unit)?;
    let n = match unit.months() {
        Some(per) => {
            let mut months =
                i64::from(a.year() - b.year()) * 12 + i64::from(a.month()) - i64::from(b.month());
            let reached = |m: i64| add_months(b, m).ok_or_else(|| out_of_range(name));
            if months > 0 && reached(months)? > a {
                months -= 1;
            } else if months < 0 && reached(months)? < a {
                months += 1;
            }
            months / per
        }
        None => (a - b).num_seconds() / unit.seconds(),
    };
    Ok(Value::from(n))
}

/// `now()`: the time of the render in UTC, to the second. `ctx.now` in the
/// request fixes it, for output that is the same on every run.
pub fn now(state: &State) -> Result<Value, Error> {
    if let Some(fixed) = state
        .lookup("ctx")
        .and_then(|ctx| ctx.get_attr("now").ok())
        .filter(|v| !v.is_undefined() && !v.is_none())
    {
        return Ok(parse_when("now", &fixed)
            .map_err(|e| invalid(format!("ctx.now: {}", e.detail().unwrap_or("invalid"))))?
            .into_value());
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| invalid("now: the clock is before 1970"))?
        .as_secs();
    let dt = i64::try_from(secs)
        .ok()
        .and_then(|s| DateTime::from_timestamp(s, 0))
        .ok_or_else(|| out_of_range("now"))?;
    Ok(When::Zoned(dt.fixed_offset()).into_value())
}

// ---------------------------------------------------------------- durations

/// Milliseconds in an ISO 8601 duration such as `P1Y2M3DT4H5M6.5S`. A year
/// counts 365 days and a month 30.
fn parse_iso_duration(s: &str) -> Option<f64> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        let n = r"(\d+(?:[.,]\d+)?)";
        regex::Regex::new(&format!(
            r"^([-+])?P(?:{n}Y)?(?:{n}M)?(?:{n}W)?(?:{n}D)?(?:T(?:{n}H)?(?:{n}M)?(?:{n}S)?)?$"
        ))
        .expect("duration pattern")
    });
    let s = s.trim().to_ascii_uppercase();
    let caps = re.captures(&s)?;
    const DAY: f64 = 86_400_000.0;
    let scales = [
        365.0 * DAY,
        30.0 * DAY,
        7.0 * DAY,
        DAY,
        3_600_000.0,
        60_000.0,
        1000.0,
    ];
    let mut total = 0.0;
    let mut any = false;
    for (i, scale) in scales.iter().enumerate() {
        if let Some(m) = caps.get(i + 2) {
            total += m.as_str().replace(',', ".").parse::<f64>().ok()? * scale;
            any = true;
        }
    }
    if !any || s.ends_with('T') {
        return None;
    }
    Some(if caps.get(1).is_some_and(|m| m.as_str() == "-") {
        -total
    } else {
        total
    })
}

/// `duration(unit="human")`: an interval in milliseconds, or an ISO 8601
/// duration, as a number of `unit` or, by default, in words.
pub fn duration(value: &Value, unit: Option<&str>) -> Result<Value, Error> {
    let ms = if let Some(s) = value.as_str() {
        parse_iso_duration(s).ok_or_else(|| {
            invalid(format!(
                "duration: cannot parse {s:?}; use milliseconds or ISO 8601 such as P1DT2H"
            ))
        })?
    } else if value.is_number() {
        f64::try_from(value.clone())?
    } else {
        return Err(invalid(format!(
            "duration: expected milliseconds or an ISO 8601 duration, got {}",
            value.kind()
        )));
    };
    if !ms.is_finite() {
        return Err(invalid("duration: the interval is out of range"));
    }
    let raw = unit.unwrap_or("human").trim().to_ascii_lowercase();
    if raw == "human" {
        return Ok(Value::from(humanize(ms)));
    }
    let singular = raw.strip_suffix('s').unwrap_or(&raw);
    let per = match singular {
        "millisecond" | "m" => 1.0,
        "second" => 1000.0,
        "minute" => 60_000.0,
        "hour" => 3_600_000.0,
        "day" => 86_400_000.0,
        "week" => 7.0 * 86_400_000.0,
        "month" => 30.0 * 86_400_000.0,
        "year" => 365.0 * 86_400_000.0,
        _ => {
            return Err(invalid(format!(
                "duration: unknown unit {raw:?}; use human, years, months, weeks, days, hours, minutes, seconds or milliseconds"
            )))
        }
    };
    Ok(number(ms / per))
}

/// An interval in words, rounded the way people say it: `a few seconds`,
/// `a minute`, `5 minutes`, `an hour`, `3 hours`, `a day`, `12 days`,
/// `a month`, `4 months`, `a year`, `2 years`. The sign is ignored.
fn humanize(ms: f64) -> String {
    let seconds = (ms.abs() / 1000.0).round();
    let minutes = (seconds / 60.0).round();
    let hours = (minutes / 60.0).round();
    let days = (hours / 24.0).round();
    if seconds < 45.0 {
        return "a few seconds".into();
    }
    if seconds < 90.0 {
        return "a minute".into();
    }
    if minutes < 45.0 {
        return format!("{minutes} minutes");
    }
    if minutes < 90.0 {
        return "an hour".into();
    }
    if hours < 22.0 {
        return format!("{hours} hours");
    }
    if hours < 36.0 {
        return "a day".into();
    }
    if days < 26.0 {
        return format!("{days} days");
    }
    let months = (days / 30.436_875).round();
    if months <= 1.0 {
        return "a month".into();
    }
    if months < 11.0 {
        return format!("{months} months");
    }
    let years = (days / 365.25).round();
    if years <= 1.0 {
        "a year".into()
    } else {
        format!("{years} years")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::context;

    fn env(lang: Option<&str>) -> Environment<'static> {
        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::Strict);
        add_filters(&mut env, lang);
        env
    }

    fn render(src: &str, data: Value) -> String {
        env(None).render_str(src, data).unwrap()
    }

    fn fails(src: &str, data: Value) -> String {
        let e = env(None).render_str(src, data).unwrap_err();
        format!("{e:#}")
    }

    #[test]
    fn empty_test() {
        let data = context! {
            n => (), s => "", l => Vec::<i32>::new(), m => context! {},
            zero => 0, f => false, x => "x", one => vec![1], k => context! { a => 1 },
        };
        let src = "{{ n is empty }} {{ missing is empty }} {{ s is empty }} {{ l is empty }} \
                   {{ m is empty }} {{ zero is empty }} {{ f is empty }} {{ x is empty }} \
                   {{ one is empty }} {{ k is empty }} {{ x is not empty }} {{ range(0) is empty }}";
        assert_eq!(
            render(src, data),
            "True True True True True False False False False False True True"
        );
    }

    #[test]
    fn truncates_by_characters() {
        let s = "crème brûlée au café";
        assert_eq!(truncate_str(s, 100, "…", false), s);
        assert_eq!(truncate_str(s, 20, "…", false), s);
        assert_eq!(truncate_str(s, 9, "…", false), "crème br…");
        assert_eq!(truncate_str(s, 9, "…", true), "crème…");
        assert_eq!(truncate_str(s, 13, "...", true), "crème...");
        // The cut falls on a space: the whole first two words fit.
        assert_eq!(truncate_str(s, 13, "…", true), "crème brûlée…");
        // A first word longer than the room leaves only the end.
        assert_eq!(truncate_str("antidisestablishment", 8, "…", true), "…");
        assert_eq!(truncate_str("abcdef", 2, "...", false), "..");
        assert_eq!(truncate_str("abcdef", 0, "…", false), "");
        assert_eq!(truncate_str("", 3, "…", false), "");
        assert_eq!(
            render(
                r#"{{ s | truncate(9) }}|{{ s | truncate(9, end="") }}|{{ s | truncate(9, "!", words=true) }}|{{ 123456 | truncate(4, end="") }}|{{ n | truncate(2) }}"#,
                context! { s => s, n => () }
            ),
            "crème br…|crème brû|crème!|1234|"
        );
        assert!(fails("{{ 'a' | truncate(-1) }}", context! {}).contains("0 or more"));
        assert!(fails("{{ [1] | truncate(3) }}", context! {}).contains("expected text"));
        assert!(fails("{{ 'a' | truncate(3, wordz=true) }}", context! {}).contains("wordz"));
    }

    #[test]
    fn unaccents() {
        let out = render(
            "{{ a | unaccent }}|{{ b | unaccent }}|{{ c | unaccent }}|{{ d | unaccent }}",
            context! {
                a => "crème brulée",
                b => "Ærøskøbing Straße, Łódź, ÉCOLE, Ñandú",
                c => "हिन्दी 한국어 がぎ",
                d => "e\u{301}",
            },
        );
        assert_eq!(
            out,
            "creme brulee|AEroskobing Strasse, Lodz, ECOLE, Nandu|हिन्दी 한국어 がぎ|e"
        );
    }

    #[test]
    fn pads_by_characters() {
        let out = render(
            r#"[{{ 42 | pad_left(6, "0") }}][{{ "é" | pad_right(3, char=".") }}][{{ "abcdef" | pad_left(3) }}][{{ "ü" | pad_left(3) }}][{{ "x" | pad_right(3, "€") }}][{{ n | pad_left(2, "-") }}]"#,
            context! { n => () },
        );
        assert_eq!(out, "[000042][é..][abcdef][  ü][x€€][--]");
        assert!(fails("{{ 'a' | pad_left(100000) }}", context! {}).contains("between 0 and 10000"));
        assert!(fails("{{ 'a' | pad_left(-1) }}", context! {}).contains("between 0 and 10000"));
        assert!(fails("{{ 'a' | pad_right(3, 'ab') }}", context! {}).contains("one character"));
        assert!(fails("{{ 'a' | pad_right(3, '') }}", context! {}).contains("one character"));
    }

    #[test]
    fn averages() {
        let data = context! {
            l => vec![1, 2, 4],
            f => vec![1.5, 2.5],
            o => vec![
                context! { p => 10, q => context! { r => 1 } },
                context! { p => 20 },
                context! { p => () },
                context! { q => context! { r => 3 } },
            ],
            e => Vec::<i32>::new(),
        };
        assert_eq!(
            render(
                "{{ l | avg | round(2) }}|{{ f | avg }}|{{ o | avg(attribute='p') }}|{{ o | avg('q.r') }}|{{ e | avg is none }}|{{ o | avg(attribute='nope') is none }}",
                data
            ),
            "2.33|2|15|2|True|True"
        );
        assert!(fails("{{ ['a'] | avg }}", context! {}).contains("only average numbers"));
        assert!(fails("{{ nope | avg }}", context! {}).contains("undefined"));
    }

    #[test]
    fn ceil_and_floor() {
        assert_eq!(
            render(
                "{{ 2.1 | ceil }} {{ 2.9 | floor }} {{ -2.1 | ceil }} {{ -2.1 | floor }} {{ 3 | ceil }} {{ (7 / 2) | floor }}",
                context! {}
            ),
            "3 2 -2 -3 3 3"
        );
        assert!(fails("{{ 'x' | ceil }}", context! {}).contains("expected a number"));
        assert!(fails("{{ 1e300 | floor }}", context! {}).contains("out of range"));
    }

    #[test]
    fn spells_numbers() {
        assert_eq!(
            render(
                "{{ 1234 | words }}|{{ 0 | words }}|{{ -7 | words }}|{{ 1.5 | words }}|{{ 42 | words(ordinal=true) }}|{{ 12.29 | words(currency='eur') }}|{{ 1 | words(currency='USD') }}|{{ 2001 | words }}|{{ 1000105 | words(currency='DOLLAR') }}",
                context! {}
            ),
            "one thousand two hundred thirty-four|zero|minus seven|one point five|forty-second|twelve euros and twenty-nine cents|one US dollar|two thousand one|one million one hundred five dollars"
        );
        assert_eq!(
            env(Some("fr-FR"))
                .render_str("{{ 81 | words }}|{{ 81 | words(lang='en') }}", context! {})
                .unwrap(),
            "quatre-vingt-un|eighty-one"
        );
        assert_eq!(
            env(Some("fr_BE"))
                .render_str("{{ 70 | words }}", context! {})
                .unwrap(),
            "septante"
        );
        // A language with no words falls back to English.
        assert_eq!(
            env(Some("de"))
                .render_str("{{ 3 | words }}", context! {})
                .unwrap(),
            "three"
        );
        assert!(fails("{{ 3 | words(lang='de') }}", context! {}).contains("no words for language"));
        assert!(fails("{{ 1e15 | words }}", context! {}).contains("too large"));
        assert!(fails("{{ 'x' | words }}", context! {}).contains("expected a number"));
        assert!(
            fails("{{ 1 | words(currency='XYZ') }}", context! {}).contains("no words for currency")
        );
        assert!(
            fails("{{ 1 | words(ordinal=true, currency='EUR') }}", context! {})
                .contains("not both")
        );
        assert!(fails("{{ -1 | words(ordinal=true) }}", context! {}).contains("negative"));
        assert!(fails("{{ 1.5 | words(ordinal=true) }}", context! {}).contains("float"));
    }

    #[test]
    fn adds_calendar_units() {
        let cases = [
            ("2024-01-31", 1, "month", "2024-02-29"),
            ("2023-01-31", 1, "month", "2023-02-28"),
            ("2024-02-29", 1, "year", "2025-02-28"),
            ("2024-02-29", 4, "years", "2028-02-29"),
            ("2024-03-31", -1, "month", "2024-02-29"),
            ("2024-11-30", 1, "quarter", "2025-02-28"),
            ("2024-12-25", 2, "week", "2025-01-08"),
            ("2024-02-28", 1, "day", "2024-02-29"),
            ("2024-02-28", 30, "hour", "2024-02-29T06:00:00"),
            ("2024-02-28T23:59:30", 45, "second", "2024-02-29T00:00:15"),
            (
                "2024-03-30T12:00:00+02:00",
                1,
                "day",
                "2024-03-31T12:00:00+02:00",
            ),
            ("2024-10-27T01:30:00Z", 1, "hour", "2024-10-27T02:30:00Z"),
            ("2024-10-27 01:30:00", 90, "minutes", "2024-10-27T03:00:00"),
        ];
        for (from, n, unit, want) in cases {
            let got = render(
                "{{ d | date_add(n, u) }}",
                context! { d => from, n => n, u => unit },
            );
            assert_eq!(got, want, "{from} + {n} {unit}");
        }
        assert_eq!(
            render(
                "{{ 0 | date_add(1) }}|{{ '2024-03-31' | date_sub(1, 'month') }}|{{ 86400.5 | date_add(1, 'day') }}",
                context! {}
            ),
            "1970-01-02T00:00:00Z|2024-02-29|1970-01-03T00:00:00.500Z"
        );
    }

    #[test]
    fn refuses_dates_out_of_range() {
        for src in [
            "{{ '2024-01-01' | date_add(9223372036854775807, 'year') }}",
            "{{ '2024-01-01' | date_add(300000, 'year') }}",
            "{{ '2024-01-01' | date_add(-9223372036854775807, 'second') }}",
            "{{ '2024-01-01' | date_sub(-9223372036854775807 - 1) }}",
            "{{ 1e300 | date_add(1) }}",
            "{{ 8210298412799 | end_of('year') }}",
        ] {
            let e = fails(src, context! {});
            assert!(e.contains("out of range"), "{src}: {e}");
        }
        assert!(fails("{{ 'soon' | date_add(1) }}", context! {}).contains("cannot parse"));
        assert!(
            fails("{{ '2024-01-01' | date_add(1, 'fortnight') }}", context! {})
                .contains("unknown unit")
        );
        assert!(fails("{{ [1] | start_of('day') }}", context! {}).contains("expected a date"));
    }

    #[test]
    fn boundaries() {
        let cases = [
            ("2024-02-14", "month", "2024-02-01", "2024-02-29"),
            ("2023-02-14", "month", "2023-02-01", "2023-02-28"),
            ("2024-08-14", "quarter", "2024-07-01", "2024-09-30"),
            ("2024-08-14", "year", "2024-01-01", "2024-12-31"),
            // 14 November 2024 is a Thursday.
            ("2024-11-14", "week", "2024-11-11", "2024-11-17"),
            ("2024-12-31", "week", "2024-12-30", "2025-01-05"),
            ("2024-11-14", "day", "2024-11-14", "2024-11-14"),
            (
                "2024-11-14",
                "hour",
                "2024-11-14T00:00:00",
                "2024-11-14T00:59:59",
            ),
            (
                "2024-11-14T10:20:30.5",
                "day",
                "2024-11-14T00:00:00",
                "2024-11-14T23:59:59",
            ),
            (
                "2024-11-14T10:20:30+05:30",
                "month",
                "2024-11-01T00:00:00+05:30",
                "2024-11-30T23:59:59+05:30",
            ),
            (
                "2024-11-14T10:20:30Z",
                "minute",
                "2024-11-14T10:20:00Z",
                "2024-11-14T10:20:59Z",
            ),
        ];
        for (d, unit, start, end) in cases {
            let got = render(
                "{{ d | start_of(u) }} {{ d | end_of(u) }}",
                context! { d => d, u => unit },
            );
            assert_eq!(got, format!("{start} {end}"), "{d} {unit}");
        }
    }

    #[test]
    fn differences() {
        let cases = [
            ("2024-03-01", "2024-02-01", "day", 29),
            ("2023-03-01", "2023-02-01", "day", 28),
            ("2024-02-01", "2024-03-01", "days", -29),
            ("2024-02-29", "2024-01-31", "month", 1),
            ("2024-02-28", "2024-01-31", "month", 0),
            ("2024-03-31", "2024-01-31", "month", 2),
            ("2025-02-28", "2024-02-29", "year", 1),
            ("2025-02-27", "2024-02-29", "year", 0),
            ("2025-03-01", "2024-02-29", "year", 1),
            ("2024-01-01", "2025-01-02", "year", -1),
            ("2024-01-01", "2024-12-31", "year", 0),
            ("2024-12-31", "2024-01-01", "quarter", 3),
            ("2024-01-15", "2024-01-01", "week", 2),
            (
                "2024-01-01T10:00:00Z",
                "2024-01-01T12:30:00+02:00",
                "minute",
                -30,
            ),
            ("2024-01-01T00:00:00", "2023-12-31T23:59:59", "second", 1),
            ("2024-01-01T00:00:00", "2023-12-31T23:59:59", "day", 0),
            ("2024-01-01T23:00:00", "2024-01-01", "hours", 23),
        ];
        for (a, b, unit, want) in cases {
            let got = render(
                "{{ a | date_diff(b, u) }}",
                context! { a => a, b => b, u => unit },
            );
            assert_eq!(got, want.to_string(), "{a} - {b} in {unit}");
        }
        assert_eq!(
            render("{{ '2024-01-03' | date_diff('2024-01-01') }}", context! {}),
            "2"
        );
    }

    #[test]
    fn now_is_utc_or_fixed() {
        let out = render("{{ now() }}", context! {});
        let parsed = DateTime::parse_from_rfc3339(&out).unwrap();
        assert_eq!(parsed.offset().local_minus_utc(), 0);
        assert!(out.ends_with('Z'), "{out}");
        let clock = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!((clock - parsed.timestamp()).abs() < 5);
        assert_eq!(
            render(
                "{{ now() }}|{{ now() | date_add(1, 'month') }}",
                context! { ctx => context! { now => "2024-01-31T09:00:00Z" } }
            ),
            "2024-01-31T09:00:00Z|2024-02-29T09:00:00Z"
        );
        assert_eq!(
            render(
                "{{ now() }}",
                context! { ctx => context! { now => "2024-01-31" } }
            ),
            "2024-01-31"
        );
        assert!(fails(
            "{{ now() }}",
            context! { ctx => context! { now => "later" } }
        )
        .contains("ctx.now"));
        // A ctx with no `now` is the clock.
        assert!(render("{{ now() }}", context! { ctx => context! { a => 1 } }).ends_with('Z'));
    }

    #[test]
    fn durations() {
        let day = 86_400_000_i64;
        let cases: Vec<(Value, &str, &str)> = vec![
            (Value::from(7_200_000), "human", "2 hours"),
            (Value::from(3_000), "human", "a few seconds"),
            (Value::from(60_000), "human", "a minute"),
            (Value::from(300_000), "human", "5 minutes"),
            (Value::from(3_600_000), "human", "an hour"),
            (Value::from(-day), "human", "a day"),
            (Value::from(10 * day), "human", "10 days"),
            (Value::from(30 * day), "human", "a month"),
            (Value::from(120 * day), "human", "4 months"),
            (Value::from(365 * day), "human", "a year"),
            (Value::from(3 * 365 * day), "human", "3 years"),
            (Value::from(5_400_000), "hours", "1.5"),
            (Value::from(5_400_000), "minutes", "90"),
            (Value::from("PT1H30M"), "minutes", "90"),
            (Value::from("P1Y2M3DT4H"), "days", "428.1666666666667"),
            (Value::from("P2W"), "day", "14"),
            (Value::from("-PT0,5S"), "ms", "-500"),
        ];
        for (v, unit, want) in cases {
            let got = render(
                "{{ v | duration(u) }}",
                context! { v => v.clone(), u => unit },
            );
            assert_eq!(got, want, "{v} in {unit}");
        }
        assert_eq!(render("{{ 7200000 | duration }}", context! {}), "2 hours");
        for bad in ["P", "PT", "P1DT", "2 hours"] {
            assert!(
                fails("{{ v | duration }}", context! { v => bad }).contains("cannot parse"),
                "{bad}"
            );
        }
        assert!(fails("{{ 1 | duration('fortnights') }}", context! {}).contains("unknown unit"));
        assert!(fails("{{ [] | duration }}", context! {}).contains("expected milliseconds"));
    }
}
