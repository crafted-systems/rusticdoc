//! The document filters: `currency`, `number`, `date` and `t`, and the
//! minijinja environment that carries them. The rich filters live in
//! `crate::rich`.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::sync::Arc;

use chrono::format::{Fixed, Item, Numeric, StrftimeItems};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use minijinja::value::Value;
use minijinja::{AutoEscape, Environment, Error, ErrorKind, UndefinedBehavior};

use crate::locale::{self, DateNames, Decimals, LocaleData, Zone};
use crate::rich::{self, Mode};
use crate::translate::Translator;
use crate::{RenderOptions, Settings};

/// `{{ 1234.5 | currency("EUR") }}` gives `EUR 1,234.50`. Without a code the
/// template's `{% settings currency=... %}` applies.
pub fn currency(value: f64, code: Option<String>) -> String {
    let negative = value < 0.0;
    let cents = (value.abs() * 100.0).round() as u64;
    let whole = cents / 100;
    let frac = cents % 100;
    let digits = whole.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let sign = if negative { "-" } else { "" };
    match code {
        Some(code) if !code.is_empty() => format!("{sign}{code} {grouped}.{frac:02}"),
        _ => format!("{sign}{grouped}.{frac:02}"),
    }
}

/// `{{ d | date("%d/%m/%Y") }}` formats an ISO 8601 string or unix seconds.
/// Values with a zone, and unix times, are shown in `timezone` (an offset
/// such as `+02:00`, or an IANA zone such as `Europe/Paris`; UTC otherwise).
/// The default format is `%Y-%m-%d`. Names are English.
pub fn date(value: &Value, format: Option<&str>, timezone: Option<&str>) -> Result<String, Error> {
    date_in(value, format, timezone, None)
}

/// [`date`] with month and day names from `names`, when given.
pub(crate) fn date_in(
    value: &Value,
    format: Option<&str>,
    timezone: Option<&str>,
    names: Option<&DateNames>,
) -> Result<String, Error> {
    let fmt = format.unwrap_or("%Y-%m-%d");
    let zone = Zone::parse(timezone.unwrap_or("UTC"))
        .map_err(|e| Error::new(ErrorKind::InvalidOperation, e))?;
    let naive: NaiveDateTime = if let Some(s) = value.as_str() {
        let s = s.trim();
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            zone.local(&dt)
        } else if let Ok(dt) = DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%z") {
            zone.local(&dt)
        } else if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
            dt
        } else if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
            dt
        } else if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
            dt
        } else if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            d.and_hms_opt(0, 0, 0).unwrap_or_default()
        } else {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                format!("date: cannot parse {s:?}; use ISO 8601 or unix seconds"),
            ));
        }
    } else if let Some(n) = value.as_i64() {
        unix(n as f64, &zone)?
    } else if let Ok(n) = f64::try_from(value.clone()) {
        unix(n, &zone)?
    } else {
        return Err(Error::new(
            ErrorKind::InvalidOperation,
            format!("date: expected a string or a number, got {}", value.kind()),
        ));
    };
    let invalid = || {
        Error::new(
            ErrorKind::InvalidOperation,
            format!("date: invalid format {fmt:?}"),
        )
    };
    let mut out = String::new();
    match names {
        None => write!(out, "{}", naive.format(fmt)).map_err(|_| invalid())?,
        Some(names) => {
            let items = localized_items(fmt, names, &naive).ok_or_else(invalid)?;
            write!(out, "{}", naive.format_with_items(items.iter())).map_err(|_| invalid())?
        }
    }
    Ok(out)
}

/// The format's items with month, weekday and AM/PM names in the language.
/// A month name reads as it does in a date ("5 января") when the format has
/// a day of the month, and as it does alone ("январь") otherwise.
fn localized_items<'a>(
    fmt: &'a str,
    names: &DateNames,
    when: &NaiveDateTime,
) -> Option<Vec<Item<'a>>> {
    use chrono::{Datelike, Timelike};
    let items: Vec<Item<'a>> = StrftimeItems::new(fmt).collect();
    if items.iter().any(|i| matches!(i, Item::Error)) {
        return None;
    }
    let in_date = items
        .iter()
        .any(|i| matches!(i, Item::Numeric(Numeric::Day, _)));
    let month = when.month0() as usize;
    let weekday = when.weekday().num_days_from_monday() as usize;
    let (wide, short) = if in_date { (0, 1) } else { (2, 3) };
    let am_pm = names.am_pm[usize::from(when.hour() >= 12)];
    Some(
        items
            .into_iter()
            .map(|item| {
                let text = match &item {
                    Item::Fixed(Fixed::LongMonthName) => names.months[wide][month].to_string(),
                    Item::Fixed(Fixed::ShortMonthName) => names.months[short][month].to_string(),
                    Item::Fixed(Fixed::LongWeekdayName) => names.weekdays[0][weekday].to_string(),
                    Item::Fixed(Fixed::ShortWeekdayName) => names.weekdays[1][weekday].to_string(),
                    Item::Fixed(Fixed::UpperAmPm) => am_pm.to_string(),
                    Item::Fixed(Fixed::LowerAmPm) => am_pm.to_lowercase(),
                    _ => return item,
                };
                Item::OwnedLiteral(text.into())
            })
            .collect(),
    )
}

fn unix(secs: f64, zone: &Zone) -> Result<NaiveDateTime, Error> {
    let whole = secs.floor();
    let nanos = ((secs - whole) * 1e9) as u32;
    Utc.timestamp_opt(whole as i64, nanos)
        .single()
        .map(|dt| zone.local(&dt))
        .ok_or_else(|| Error::new(ErrorKind::InvalidOperation, "date: time out of range"))
}

/// Most decimals `number` writes.
const MAX_DECIMALS: i64 = 20;

/// `{{ 1234.5 | number }}` gives `1,234.5`, and `number(2)` gives
/// `1,234.50`: up to three decimals, or exactly as many as asked, with the
/// language's separators when it has formatting data.
pub(crate) fn number(
    value: f64,
    decimals: Option<i64>,
    locale: Option<&LocaleData>,
) -> Result<String, Error> {
    let decimals = match decimals {
        None => Decimals::Upto3,
        Some(n) if (0..=MAX_DECIMALS).contains(&n) => Decimals::Exactly(n as usize),
        Some(n) => {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                format!("number: decimals must be from 0 to {MAX_DECIMALS}, not {n}"),
            ))
        }
    };
    let data = locale.map_or(&locale::PLAIN, |l| &l.number);
    Ok(locale::format_number(data, value, decimals))
}

/// Characters XML 1.0 forbids; they are dropped from every value, which also
/// keeps the part separator (NUL) out of rendered text.
pub fn xml_illegal(c: char) -> bool {
    matches!(c, '\0'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}')
}

fn write_xml_escaped(
    out: &mut minijinja::Output,
    s: &str,
    breaks: &str,
    escape: bool,
) -> std::fmt::Result {
    for c in s.chars() {
        match c {
            c if xml_illegal(c) => {}
            '&' if escape => out.write_str("&amp;")?,
            '<' if escape => out.write_str("&lt;")?,
            '>' if escape => out.write_str("&gt;")?,
            '\n' if escape && !breaks.is_empty() => out.write_str(breaks)?,
            c => out.write_char(c)?,
        }
    }
    Ok(())
}

/// The expression environment for a template with these settings. Output is
/// XML-escaped; `breaks` is the markup a newline in a value becomes. The rich
/// filters are present but refuse to run: there is no package to put their
/// content in.
pub fn environment_for(settings: &Settings, breaks: &'static str) -> Environment<'static> {
    office_environment(settings, breaks, Mode::Text)
}

/// [`environment_for`] with the rich filters in the given mode.
pub(crate) fn office_environment(
    settings: &Settings,
    breaks: &'static str,
    mode: Mode,
) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_fuel(Some(5_000_000));
    env.set_auto_escape_callback(|_| AutoEscape::Custom(Cow::Borrowed("xml")));
    env.set_formatter(move |out, state, value| {
        if let Some(token) = value.downcast_object_ref::<rich::Token>() {
            return rich::write_token(out, token, mode);
        }
        if !matches!(state.auto_escape(), AutoEscape::Custom(c) if c == "xml") {
            // Turning escaping off must not let a value forge a marker.
            return minijinja::escape_formatter(out, state, &rich::scrubbed(value));
        }
        if value.is_undefined() || value.is_none() {
            return Ok(());
        }
        write_xml_escaped(out, &value.to_string(), breaks, !value.is_safe()).map_err(Error::from)
    });
    add_document_filters(
        &mut env,
        Arc::new(Context::new(settings, &RenderOptions::default())),
        false,
    );
    // What makes a typed sheet cell empty. A render that prints undefined
    // values as empty widens it to undefined values; see
    // [`undefined_as_empty`].
    env.add_test("__blank", |v: Value| v.is_none());
    rich::add_filters(&mut env, mode);
    env
}

/// Everything the document filters read for one render: the template's
/// settings with the request's on top.
#[derive(Debug, Clone, Default)]
pub(crate) struct Context {
    timezone: Option<String>,
    currency: Option<String>,
    /// Formatting data, when the language has some.
    locale: Option<&'static LocaleData>,
    translator: Translator,
}

impl Context {
    pub(crate) fn new(settings: &Settings, options: &RenderOptions) -> Self {
        let translator = Translator::new(settings, options);
        Self {
            timezone: options
                .timezone
                .clone()
                .or_else(|| settings.timezone.clone()),
            currency: options
                .currency
                .clone()
                .or_else(|| settings.currency.clone()),
            locale: translator.lang().and_then(locale::find),
            translator,
        }
    }

    fn currency(&self, value: f64, code: Option<String>) -> String {
        let code = code.or_else(|| self.currency.clone());
        match self.locale {
            // No language keeps the output `currency` has always had.
            None => currency(value, code),
            Some(l) => locale::format_currency(&l.number, value, code.as_deref().unwrap_or("")),
        }
    }

    fn date(&self, value: &Value, format: Option<&str>) -> Result<String, Error> {
        date_in(
            value,
            format,
            self.timezone.as_deref(),
            self.locale.map(|l| l.dates),
        )
    }
}

/// Adds `currency`, `number`, `date` and `t` reading `ctx`. When `lenient`,
/// each passes an undefined value through so that it prints as nothing.
pub(crate) fn add_document_filters(
    env: &mut Environment<'static>,
    ctx: Arc<Context>,
    lenient: bool,
) {
    let c = ctx.clone();
    env.add_filter(
        "currency",
        move |value: Value, code: Option<String>| -> Result<Value, Error> {
            if lenient && value.is_undefined() {
                return Ok(value);
            }
            Ok(Value::from(c.currency(f64::try_from(value)?, code)))
        },
    );
    let c = ctx.clone();
    env.add_filter(
        "number",
        move |value: Value, decimals: Option<i64>| -> Result<Value, Error> {
            if lenient && value.is_undefined() {
                return Ok(value);
            }
            number(f64::try_from(value)?, decimals, c.locale).map(Value::from)
        },
    );
    let c = ctx.clone();
    env.add_filter(
        "date",
        move |value: Value, format: Option<String>| -> Result<Value, Error> {
            if lenient && value.is_undefined() {
                return Ok(value);
            }
            c.date(&value, format.as_deref()).map(Value::from)
        },
    );
    env.add_filter("t", move |key: Value| -> Result<Value, Error> {
        if lenient && key.is_undefined() {
            return Ok(key);
        }
        let key = key.as_str().map_or_else(|| key.to_string(), str::to_string);
        Ok(Value::from(ctx.translator.translate(&key)))
    });
}

/// Puts a render's settings into an environment made by
/// [`office_environment`], in place of the template's alone.
pub(crate) fn localize(env: &mut Environment<'static>, ctx: Context) {
    add_document_filters(env, Arc::new(ctx), false);
}

/// Turns an environment made by [`office_environment`] into one that prints
/// an undefined value, or a missing attribute of one, as nothing. The
/// filters that need a value (`date`, `currency`, `number`, `t`, `round`,
/// `abs`) pass an undefined value through instead of failing, so it prints
/// as nothing too, and a sheet cell holding one is left empty.
pub(crate) fn undefined_as_empty(env: &mut Environment<'static>, ctx: Context) {
    env.set_undefined_behavior(UndefinedBehavior::Chainable);
    env.add_test("__blank", |v: Value| v.is_none() || v.is_undefined());
    add_document_filters(env, Arc::new(ctx), true);
    env.add_filter(
        "round",
        |value: Value,
         precision: Option<i32>,
         method: Option<&str>,
         kwargs: minijinja::value::Kwargs|
         -> Result<Value, Error> {
            if value.is_undefined() {
                return Ok(value);
            }
            minijinja::filters::round(value, precision, method, kwargs)
        },
    );
    env.add_filter("abs", |value: Value| -> Result<Value, Error> {
        if value.is_undefined() {
            return Ok(value);
        }
        minijinja::filters::abs(value)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn currency_formats() {
        assert_eq!(currency(0.0, None), "0.00");
        assert_eq!(
            currency(-1234567.891, Some("USD".into())),
            "-USD 1,234,567.89"
        );
        assert_eq!(currency(999.999, Some("".into())), "1,000.00");
    }

    #[test]
    fn dates_parse_many_shapes() {
        let f = |v: Value, fmt: Option<&str>, tz: Option<&str>| date(&v, fmt, tz).unwrap();
        assert_eq!(f("2024-03-05".into(), Some("%d/%m/%Y"), None), "05/03/2024");
        assert_eq!(
            f(
                "2024-03-05T23:30:00Z".into(),
                Some("%Y-%m-%d %H:%M"),
                Some("+02:00")
            ),
            "2024-03-06 01:30"
        );
        assert_eq!(
            f("2024-03-05T10:20:30.123".into(), Some("%H:%M:%S"), None),
            "10:20:30"
        );
        assert_eq!(f(Value::from(0), None, None), "1970-01-01");
        assert_eq!(f(Value::from(86400.5), Some("%j"), None), "002");
        assert!(date(&"yesterday".into(), None, None).is_err());
        assert!(date(&"2024-03-05".into(), None, Some("Mars/Olympus")).is_err());
    }

    #[test]
    fn environment_escapes_and_translates() {
        let mut settings = Settings::default();
        settings.translations.insert("hello".into(), "hallo".into());
        settings
            .translations
            .insert("hello.fr".into(), "bonjour".into());
        settings.lang = Some("fr".into());
        settings.currency = Some("EUR".into());
        let env = environment_for(&settings, "<br/>");
        let out = env
            .render_str(
                "{{ x }}|{{ 'hello' | t }}|{{ 'bye' | t }}|{{ 2 | currency }}|{{ y | safe }}",
                minijinja::context! { x => "a<b&c\nd", y => "<r/>" },
            )
            .unwrap();
        assert_eq!(out, "a&lt;b&amp;c<br/>d|bonjour|bye|2,00\u{a0}€|<r/>");
    }
}
