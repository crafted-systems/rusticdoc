//! The document filters: `currency`, `date` and `t`, and the minijinja
//! environment that carries them.

use std::borrow::Cow;
use std::fmt::Write as _;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};
use minijinja::value::Value;
use minijinja::{AutoEscape, Environment, Error, ErrorKind, UndefinedBehavior};

use crate::Settings;

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
/// Values with a zone, and unix times, are shown in `timezone` (a fixed
/// offset such as `+02:00`; UTC otherwise). The default format is
/// `%Y-%m-%d`.
pub fn date(value: &Value, format: Option<&str>, timezone: Option<&str>) -> Result<String, Error> {
    let fmt = format.unwrap_or("%Y-%m-%d");
    let zone = parse_offset(timezone.unwrap_or("UTC"))?;
    let naive: NaiveDateTime = if let Some(s) = value.as_str() {
        let s = s.trim();
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            dt.with_timezone(&zone).naive_local()
        } else if let Ok(dt) = DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%z") {
            dt.with_timezone(&zone).naive_local()
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
    let mut out = String::new();
    write!(out, "{}", naive.format(fmt)).map_err(|_| {
        Error::new(
            ErrorKind::InvalidOperation,
            format!("date: invalid format {fmt:?}"),
        )
    })?;
    Ok(out)
}

fn unix(secs: f64, zone: &FixedOffset) -> Result<NaiveDateTime, Error> {
    let whole = secs.floor();
    let nanos = ((secs - whole) * 1e9) as u32;
    Utc.timestamp_opt(whole as i64, nanos)
        .single()
        .map(|dt| dt.with_timezone(zone).naive_local())
        .ok_or_else(|| Error::new(ErrorKind::InvalidOperation, "date: time out of range"))
}

fn parse_offset(tz: &str) -> Result<FixedOffset, Error> {
    let tz = tz.trim();
    if tz.is_empty()
        || tz.eq_ignore_ascii_case("utc")
        || tz == "Z"
        || tz.eq_ignore_ascii_case("gmt")
    {
        return Ok(FixedOffset::east_opt(0).expect("zero offset"));
    }
    let (sign, rest) = match tz.chars().next() {
        Some('+') => (1, &tz[1..]),
        Some('-') => (-1, &tz[1..]),
        _ => {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                format!(
                "timezone {tz:?} is not a fixed offset like +02:00 (named zones are not supported)"
            ),
            ))
        }
    };
    let (h, m) = rest.split_once(':').unwrap_or((rest, "0"));
    let secs = h.parse::<i32>().unwrap_or(0) * 3600 + m.parse::<i32>().unwrap_or(0) * 60;
    FixedOffset::east_opt(sign * secs)
        .ok_or_else(|| Error::new(ErrorKind::InvalidOperation, format!("bad timezone {tz:?}")))
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
/// XML-escaped; `breaks` is the markup a newline in a value becomes.
pub fn environment_for(settings: &Settings, breaks: &'static str) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_fuel(Some(5_000_000));
    env.set_auto_escape_callback(|_| AutoEscape::Custom(Cow::Borrowed("xml")));
    env.set_formatter(move |out, state, value| {
        if !matches!(state.auto_escape(), AutoEscape::Custom(c) if c == "xml") {
            return minijinja::escape_formatter(out, state, value);
        }
        if value.is_undefined() || value.is_none() {
            return Ok(());
        }
        write_xml_escaped(out, &value.to_string(), breaks, !value.is_safe()).map_err(Error::from)
    });
    let default_currency = settings.currency.clone();
    env.add_filter("currency", move |value: f64, code: Option<String>| {
        currency(value, code.or_else(|| default_currency.clone()))
    });
    let tz = settings.timezone.clone();
    env.add_filter("date", move |value: Value, format: Option<String>| {
        date(&value, format.as_deref(), tz.as_deref())
    });
    let lang = settings.lang.clone();
    let translations = settings.translations.clone();
    env.add_filter("t", move |key: String| {
        lang.as_ref()
            .and_then(|l| translations.get(&format!("{key}.{l}")))
            .or_else(|| translations.get(&key))
            .cloned()
            .unwrap_or(key)
    });
    env
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
        assert!(date(&"2024-03-05".into(), None, Some("Europe/Paris")).is_err());
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
        assert_eq!(out, "a&lt;b&amp;c<br/>d|bonjour|bye|EUR 2.00|<r/>");
    }
}
