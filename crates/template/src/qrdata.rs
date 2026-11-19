//! `qrdata`: the text a QR code carries for Wi-Fi, a contact, an email, a
//! call, a text message, a place, an event, a SEPA or Swiss QR-bill
//! payment, or a GS1 Digital Link. The formats have escaping and checks of
//! their own, and a payload that is almost right scans as plain text or
//! not at all, so they are built here rather than by hand in a template.
//!
//! Each kind takes an object; `email` and `tel` also take a plain string.
//! Unknown keys are refused, so a misspelled one is not quietly dropped.

use std::fmt::Write as _;

use minijinja::value::{Kwargs, Value, ValueKind};
use minijinja::{Environment, Error, ErrorKind};

/// The kinds, as the error for an unknown one lists them.
pub const KINDS: &str = "wifi, vcard, mecard, email, tel, sms, geo, event, epc, swiss or gs1";

/// The most bytes an EPC payload may hold (EPC069-12).
const EPC_MAX_BYTES: usize = 331;

pub fn add_filters(env: &mut Environment<'_>) {
    env.add_filter("qrdata", filter);
}

fn fail(message: impl std::fmt::Display) -> Error {
    Error::new(ErrorKind::InvalidOperation, format!("qrdata: {message}"))
}

fn filter(value: Value, kind: Option<String>, kwargs: Kwargs) -> Result<Value, Error> {
    let kind = kind.ok_or_else(|| fail(format!("name the kind: {KINDS}")))?;
    let domain: Option<String> = kwargs.get("domain").map_err(fail)?;
    kwargs.assert_all_used().map_err(fail)?;
    if domain.is_some() && kind != "gs1" {
        return Err(fail("domain is for gs1"));
    }
    if value.is_undefined() {
        return Ok(Value::UNDEFINED);
    }
    let out = match kind.as_str() {
        "wifi" => wifi(&value),
        "vcard" => vcard(&value),
        "mecard" => mecard(&value),
        "email" => email(&value),
        "tel" => tel(&value),
        "sms" => sms(&value),
        "geo" => geo(&value),
        "event" => event(&value),
        "epc" => epc(&value),
        "swiss" => swiss(&value),
        "gs1" => gs1(&value, domain.as_deref()),
        other => Err(format!("unknown kind {other:?}; use {KINDS}")),
    }
    .map_err(|e| fail(format!("{kind}: {e}")))?;
    Ok(Value::from(out))
}

// ---------------------------------------------------------------------------
// Reading the object.

fn object<'a>(v: &'a Value, keys: &[&str]) -> Result<&'a Value, String> {
    if v.kind() != ValueKind::Map {
        return Err(format!("takes an object with {}", keys.join(", ")));
    }
    for key in v.try_iter().map_err(|e| e.to_string())? {
        let key = key.to_string();
        if !keys.contains(&key.as_str()) {
            return Err(format!(
                "unknown key {key:?}; the keys are {}",
                keys.join(", ")
            ));
        }
    }
    Ok(v)
}

/// A text field: a string or number; nothing when missing, none or empty.
fn text(v: &Value, key: &str) -> Result<Option<String>, String> {
    let f = v.get_attr(key).map_err(|e| e.to_string())?;
    match f.kind() {
        ValueKind::Undefined | ValueKind::None => Ok(None),
        ValueKind::String | ValueKind::Number => {
            let s = f.to_string();
            if s.chars().any(|c| c.is_control() && c != '\n') {
                return Err(format!("{key} holds a control character"));
            }
            Ok((!s.trim().is_empty()).then(|| s.trim().to_string()))
        }
        _ => Err(format!("{key} is text")),
    }
}

fn required(v: &Value, key: &str) -> Result<String, String> {
    text(v, key)?.ok_or_else(|| format!("{key} is required"))
}

/// A field that is one text or a list of them.
fn texts(v: &Value, key: &str) -> Result<Vec<String>, String> {
    let f = v.get_attr(key).map_err(|e| e.to_string())?;
    if f.kind() == ValueKind::Seq {
        let mut out = Vec::new();
        for item in f.try_iter().map_err(|e| e.to_string())? {
            match item.kind() {
                ValueKind::String | ValueKind::Number => {
                    let s = item.to_string();
                    if !s.trim().is_empty() {
                        out.push(s.trim().to_string());
                    }
                }
                ValueKind::None | ValueKind::Undefined => {}
                _ => return Err(format!("{key} is text or a list of text")),
            }
        }
        return Ok(out);
    }
    Ok(text(v, key)?.into_iter().collect())
}

fn flag(v: &Value, key: &str) -> Result<bool, String> {
    let f = v.get_attr(key).map_err(|e| e.to_string())?;
    match f.kind() {
        ValueKind::Undefined | ValueKind::None => Ok(false),
        ValueKind::Bool => Ok(f.is_true()),
        _ => Err(format!("{key} is true or false")),
    }
}

fn number(v: &Value, key: &str) -> Result<Option<f64>, String> {
    let f = v.get_attr(key).map_err(|e| e.to_string())?;
    let n = match f.kind() {
        ValueKind::Undefined | ValueKind::None => return Ok(None),
        ValueKind::Number => f64::try_from(f).ok(),
        ValueKind::String => f.as_str().and_then(|s| s.trim().parse::<f64>().ok()),
        _ => None,
    };
    n.filter(|n| n.is_finite())
        .map(Some)
        .ok_or_else(|| format!("{key} is a number"))
}

fn max_len(key: &str, s: &str, max: usize) -> Result<(), String> {
    if s.chars().count() > max {
        return Err(format!("{key} holds at most {max} characters"));
    }
    Ok(())
}

fn one_line(key: &str, s: &str) -> Result<(), String> {
    if s.contains('\n') {
        return Err(format!("{key} is one line"));
    }
    Ok(())
}

/// An amount with two decimals, from 0.01 to 999,999,999.99.
fn amount(v: &Value) -> Result<Option<String>, String> {
    let Some(n) = number(v, "amount")? else {
        return Ok(None);
    };
    let cents = (n * 100.0).round();
    if (cents - n * 100.0).abs() > 1e-6 {
        return Err("amount has at most two decimals".into());
    }
    if !(1.0..=99_999_999_999.0).contains(&cents) {
        return Err("amount is from 0.01 to 999999999.99".into());
    }
    Ok(Some(format!("{:.2}", cents / 100.0)))
}

// ---------------------------------------------------------------------------
// Wi-Fi, contacts, messages, places and events.

/// Escapes `\ ; , : "` with a backslash, as the WIFI and MECARD formats do.
fn backslash(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | ';' | ',' | ':' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn wifi(v: &Value) -> Result<String, String> {
    let v = object(v, &["ssid", "password", "security", "hidden"])?;
    let ssid = required(v, "ssid")?;
    max_len("ssid", &ssid, 32)?;
    let password = text(v, "password")?;
    let security = match text(v, "security")?.map(|s| s.to_ascii_uppercase()) {
        None if password.is_some() => "WPA",
        None => "nopass",
        Some(s) if s == "WPA" || s == "WPA2" || s == "WPA3" => "WPA",
        Some(s) if s == "WEP" => "WEP",
        Some(s) if s == "NOPASS" || s == "NONE" => "nopass",
        Some(s) => return Err(format!("security is WPA, WEP or nopass, not {s:?}")),
    };
    let mut out = format!("WIFI:T:{security};S:{};", backslash(&ssid));
    match (&password, security) {
        (Some(_), "nopass") => return Err("an open network (nopass) has no password".into()),
        (None, "WPA" | "WEP") => return Err(format!("a {security} network needs a password")),
        (Some(p), _) => {
            max_len("password", p, 63)?;
            let _ = write!(out, "P:{};", backslash(p));
        }
        _ => {}
    }
    if flag(v, "hidden")? {
        out.push_str("H:true;");
    }
    out.push(';');
    Ok(out)
}

/// Escapes a vCard or iCalendar text value (RFC 6350, RFC 5545).
fn ical_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ',' => out.push_str("\\,"),
            ';' => out.push_str("\\;"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

fn iso_date(key: &str, s: &str) -> Result<chrono::NaiveDate, String> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| format!("{key} is a date written YYYY-MM-DD, not {s:?}"))
}

fn vcard(v: &Value) -> Result<String, String> {
    let v = object(
        v,
        &[
            "name", "first", "last", "org", "title", "phone", "email", "url", "address", "note",
            "birthday",
        ],
    )?;
    let first = text(v, "first")?;
    let last = text(v, "last")?;
    let name = match (text(v, "name")?, &first, &last) {
        (Some(n), _, _) => n,
        (None, None, None) => return Err("name, or first and last, is required".into()),
        (None, f, l) => [f.as_deref(), l.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" "),
    };
    let mut out = String::from("BEGIN:VCARD\r\nVERSION:3.0\r\n");
    let n = if first.is_some() || last.is_some() {
        format!(
            "{};{};;;",
            ical_text(last.as_deref().unwrap_or("")),
            ical_text(first.as_deref().unwrap_or(""))
        )
    } else {
        format!("{};;;;", ical_text(&name))
    };
    let _ = write!(out, "N:{n}\r\nFN:{}\r\n", ical_text(&name));
    for (key, prop) in [("org", "ORG"), ("title", "TITLE")] {
        if let Some(s) = text(v, key)? {
            let _ = write!(out, "{prop}:{}\r\n", ical_text(&s));
        }
    }
    for phone in texts(v, "phone")? {
        let _ = write!(out, "TEL:{}\r\n", phone_number(&phone)?);
    }
    for mail in texts(v, "email")? {
        check_address(&mail)?;
        let _ = write!(out, "EMAIL:{mail}\r\n");
    }
    if let Some(url) = text(v, "url")? {
        one_line("url", &url)?;
        let _ = write!(out, "URL:{url}\r\n");
    }
    let address = v.get_attr("address").map_err(|e| e.to_string())?;
    if address.kind() == ValueKind::Map {
        let a = object(
            &address,
            &["street", "city", "region", "postcode", "country"],
        )?;
        let part =
            |k: &str| -> Result<String, String> { Ok(ical_text(&text(a, k)?.unwrap_or_default())) };
        let _ = write!(
            out,
            "ADR:;;{};{};{};{};{}\r\n",
            part("street")?,
            part("city")?,
            part("region")?,
            part("postcode")?,
            part("country")?
        );
    } else if let Some(a) = text(v, "address")? {
        let _ = write!(out, "ADR:;;{};;;;\r\n", ical_text(&a));
    }
    if let Some(b) = text(v, "birthday")? {
        let _ = write!(
            out,
            "BDAY:{}\r\n",
            iso_date("birthday", &b)?.format("%Y-%m-%d")
        );
    }
    if let Some(note) = text(v, "note")? {
        let _ = write!(out, "NOTE:{}\r\n", ical_text(&note));
    }
    out.push_str("END:VCARD");
    Ok(out)
}

fn mecard(v: &Value) -> Result<String, String> {
    let v = object(
        v,
        &[
            "name", "phone", "email", "url", "address", "note", "birthday",
        ],
    )?;
    let name = required(v, "name")?;
    let mut out = format!("MECARD:N:{};", backslash(&name));
    for phone in texts(v, "phone")? {
        let _ = write!(out, "TEL:{};", phone_number(&phone)?);
    }
    for mail in texts(v, "email")? {
        check_address(&mail)?;
        let _ = write!(out, "EMAIL:{};", backslash(&mail));
    }
    for (key, prop) in [("url", "URL"), ("address", "ADR"), ("note", "NOTE")] {
        if let Some(s) = text(v, key)? {
            one_line(key, &s)?;
            let _ = write!(out, "{prop}:{};", backslash(&s));
        }
    }
    if let Some(b) = text(v, "birthday")? {
        let _ = write!(out, "BDAY:{};", iso_date("birthday", &b)?.format("%Y%m%d"));
    }
    out.push(';');
    Ok(out)
}

/// A phone number as `tel:` wants it: digits with an optional leading `+`;
/// spaces, dots, dashes and brackets are dropped.
fn phone_number(s: &str) -> Result<String, String> {
    let mut out = String::new();
    for (i, c) in s.trim().chars().enumerate() {
        match c {
            '+' if i == 0 => out.push(c),
            '0'..='9' => out.push(c),
            ' ' | '.' | '-' | '(' | ')' | '/' => {}
            _ => return Err(format!("{s:?} is not a phone number")),
        }
    }
    let digits = out.trim_start_matches('+').len();
    if !(3..=15).contains(&digits) {
        return Err(format!(
            "{s:?} is not a phone number: it needs 3 to 15 digits"
        ));
    }
    Ok(out)
}

/// An address that goes into a `mailto:` URI as it is: characters that
/// would start a query, a parameter or an escape are refused, so a value
/// cannot add a `bcc` or a body of its own.
fn check_address(s: &str) -> Result<(), String> {
    let ok = s.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty()
            && domain.contains('.')
            && !domain.starts_with('.')
            && !domain.contains('@')
    }) && !s
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || "<>,;:\"?&=%#/\\()[]".contains(c));
    if !ok {
        return Err(format!("{s:?} is not an email address"));
    }
    Ok(())
}

/// Percent-encodes everything but the unreserved characters (RFC 3986).
fn percent(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn email(v: &Value) -> Result<String, String> {
    if v.kind() == ValueKind::String {
        let to = v.to_string();
        check_address(to.trim())?;
        return Ok(format!("mailto:{}", to.trim()));
    }
    let v = object(v, &["to", "cc", "bcc", "subject", "body"])?;
    let to = texts(v, "to")?;
    if to.is_empty() {
        return Err("to is required".into());
    }
    for a in &to {
        check_address(a)?;
    }
    let mut query = Vec::new();
    for key in ["cc", "bcc"] {
        let list = texts(v, key)?;
        for a in &list {
            check_address(a)?;
        }
        if !list.is_empty() {
            query.push(format!("{key}={}", list.join(",")));
        }
    }
    for key in ["subject", "body"] {
        if let Some(s) = text(v, key)? {
            query.push(format!("{key}={}", percent(&s.replace('\n', "\r\n"))));
        }
    }
    let mut out = format!("mailto:{}", to.join(","));
    if !query.is_empty() {
        let _ = write!(out, "?{}", query.join("&"));
    }
    Ok(out)
}

fn tel(v: &Value) -> Result<String, String> {
    let number = if v.kind() == ValueKind::String {
        v.to_string()
    } else {
        required(object(v, &["number"])?, "number")?
    };
    Ok(format!("tel:{}", phone_number(&number)?))
}

fn sms(v: &Value) -> Result<String, String> {
    let v = object(v, &["number", "body"])?;
    let number = phone_number(&required(v, "number")?)?;
    let body = text(v, "body")?.unwrap_or_default();
    Ok(format!("SMSTO:{number}:{body}"))
}

/// A coordinate with at most seven decimals, about a centimetre.
fn coordinate(n: f64) -> String {
    let s = format!("{n:.7}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

fn geo(v: &Value) -> Result<String, String> {
    let v = object(v, &["lat", "lon", "alt"])?;
    let lat = number(v, "lat")?.ok_or("lat is required")?;
    let lon = number(v, "lon")?.ok_or("lon is required")?;
    if !(-90.0..=90.0).contains(&lat) {
        return Err("lat is from -90 to 90".into());
    }
    if !(-180.0..=180.0).contains(&lon) {
        return Err("lon is from -180 to 180".into());
    }
    let mut out = format!("geo:{},{}", coordinate(lat), coordinate(lon));
    if let Some(alt) = number(v, "alt")? {
        let _ = write!(out, ",{}", coordinate(alt));
    }
    Ok(out)
}

/// An event time as iCalendar writes it: a date for an all-day event,
/// UTC for a time with an offset, local time for one without.
fn ical_time(key: &str, s: &str) -> Result<(String, i64), String> {
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let at = d
            .and_hms_opt(0, 0, 0)
            .map_or(0, |t| t.and_utc().timestamp());
        return Ok((format!(";VALUE=DATE:{}", d.format("%Y%m%d")), at));
    }
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        let utc = t.with_timezone(&chrono::Utc);
        return Ok((
            format!(":{}", utc.format("%Y%m%dT%H%M%SZ")),
            utc.timestamp(),
        ));
    }
    for f in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(t) = chrono::NaiveDateTime::parse_from_str(s, f) {
            return Ok((
                format!(":{}", t.format("%Y%m%dT%H%M%S")),
                t.and_utc().timestamp(),
            ));
        }
    }
    Err(format!(
        "{key} is a date (YYYY-MM-DD) or a time (YYYY-MM-DDTHH:MM, with Z or an offset for \
         one that is not local), not {s:?}"
    ))
}

fn event(v: &Value) -> Result<String, String> {
    let v = object(v, &["title", "start", "end", "location", "description"])?;
    let title = required(v, "title")?;
    let (start, from) = ical_time("start", &required(v, "start")?)?;
    let mut out = format!(
        "BEGIN:VEVENT\r\nSUMMARY:{}\r\nDTSTART{start}\r\n",
        ical_text(&title)
    );
    if let Some(end) = text(v, "end")? {
        let (end, to) = ical_time("end", &end)?;
        if start.starts_with(";VALUE=DATE") != end.starts_with(";VALUE=DATE") {
            return Err("start and end are both dates or both times".into());
        }
        if to < from {
            return Err("end is before start".into());
        }
        let _ = write!(out, "DTEND{end}\r\n");
    }
    for (key, prop) in [("location", "LOCATION"), ("description", "DESCRIPTION")] {
        if let Some(s) = text(v, key)? {
            let _ = write!(out, "{prop}:{}\r\n", ical_text(&s));
        }
    }
    out.push_str("END:VEVENT");
    Ok(out)
}

// ---------------------------------------------------------------------------
// Payments.

/// The remainder of a string of digits and letters (A = 10 … Z = 35)
/// read as one number, modulo 97, as IBAN and ISO 11649 check it.
fn mod97(s: &str) -> Option<u32> {
    let mut r = 0u32;
    for c in s.chars() {
        let d = c.to_digit(36)?;
        r = if d >= 10 {
            (r * 100 + d) % 97
        } else {
            (r * 10 + d) % 97
        };
    }
    Some(r)
}

/// An IBAN without spaces, in capitals, with its check digits verified.
fn iban(s: &str) -> Result<String, String> {
    let compact: String = s
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    let shaped = (15..=34).contains(&compact.len())
        && compact.chars().take(2).all(|c| c.is_ascii_uppercase())
        && compact.chars().skip(2).take(2).all(|c| c.is_ascii_digit())
        && compact.chars().all(|c| c.is_ascii_alphanumeric());
    if !shaped {
        return Err(format!("{s:?} is not an IBAN"));
    }
    let moved = format!("{}{}", &compact[4..], &compact[..4]);
    if mod97(&moved) != Some(1) {
        return Err(format!("{s:?} is not an IBAN: its check digits are wrong"));
    }
    Ok(compact)
}

/// An ISO 11649 creditor reference, `RF` and two check digits first.
fn creditor_reference(s: &str) -> Result<String, String> {
    let compact: String = s
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    let shaped = compact.is_ascii()
        && (5..=25).contains(&compact.len())
        && compact.starts_with("RF")
        && compact[2..4].chars().all(|c| c.is_ascii_digit())
        && compact.chars().all(|c| c.is_ascii_alphanumeric());
    let moved = format!(
        "{}{}",
        compact.get(4..).unwrap_or(""),
        compact.get(..4).unwrap_or("")
    );
    if !shaped || mod97(&moved) != Some(1) {
        return Err(format!(
            "{s:?} is not a creditor reference: those are RF, two check digits and up to 21 \
             letters or digits"
        ));
    }
    Ok(compact)
}

fn epc(v: &Value) -> Result<String, String> {
    let v = object(
        v,
        &[
            "name",
            "iban",
            "bic",
            "amount",
            "currency",
            "purpose",
            "reference",
            "text",
            "info",
        ],
    )?;
    let name = required(v, "name")?;
    max_len("name", &name, 70)?;
    one_line("name", &name)?;
    let iban = iban(&required(v, "iban")?)?;
    let bic = match text(v, "bic")? {
        Some(b) => {
            let b = b.to_ascii_uppercase();
            let ok = b.is_ascii()
                && (b.len() == 8 || b.len() == 11)
                && b[..6].chars().all(|c| c.is_ascii_uppercase())
                && b[6..].chars().all(|c| c.is_ascii_alphanumeric());
            if !ok {
                return Err(format!(
                    "{b:?} is not a BIC: those are 8 or 11 letters and digits"
                ));
            }
            b
        }
        None => String::new(),
    };
    match text(v, "currency")? {
        None => {}
        Some(c) if c.eq_ignore_ascii_case("EUR") => {}
        Some(c) => return Err(format!("SEPA transfers are in EUR, not {c}")),
    }
    let amount = amount(v)?.map(|a| format!("EUR{a}")).unwrap_or_default();
    let purpose = match text(v, "purpose")? {
        Some(p) if p.len() == 4 && p.bytes().all(|c| c.is_ascii_alphabetic()) => {
            p.to_ascii_uppercase()
        }
        Some(p) => {
            return Err(format!(
                "purpose is a four-letter code such as GDDS, not {p:?}"
            ))
        }
        None => String::new(),
    };
    let reference = text(v, "reference")?;
    let remittance = text(v, "text")?;
    if reference.is_some() && remittance.is_some() {
        return Err("give reference or text, not both: a transfer carries one".into());
    }
    let reference = match reference {
        Some(r) if r.to_ascii_uppercase().starts_with("RF") => creditor_reference(&r)?,
        Some(r) => {
            max_len("reference", &r, 35)?;
            one_line("reference", &r)?;
            r
        }
        None => String::new(),
    };
    let remittance = remittance.unwrap_or_default();
    max_len("text", &remittance, 140)?;
    one_line("text", &remittance)?;
    let info = text(v, "info")?.unwrap_or_default();
    max_len("info", &info, 70)?;
    one_line("info", &info)?;
    let mut lines = vec![
        "BCD".to_string(),
        "002".into(),
        "1".into(),
        "SCT".into(),
        bic,
        name,
        iban,
        amount,
        purpose,
        reference,
        remittance,
        info,
    ];
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let out = lines.join("\n");
    if out.len() > EPC_MAX_BYTES {
        return Err(format!(
            "the payload is {} bytes, and an EPC code holds at most {EPC_MAX_BYTES}",
            out.len()
        ));
    }
    Ok(out)
}

/// The recursive mod-10 check digit Swiss QR references end in.
fn mod10_recursive(digits: &str) -> u32 {
    const TABLE: [u32; 10] = [0, 9, 4, 6, 8, 2, 7, 1, 3, 5];
    let carry = digits
        .chars()
        .filter_map(|c| c.to_digit(10))
        .fold(0, |carry, d| TABLE[((carry + d) % 10) as usize]);
    (10 - carry) % 10
}

/// A Swiss QR-bill address, structured (type S), as seven lines.
fn swiss_address(v: &Value, key: &str) -> Result<Option<[String; 7]>, String> {
    let a = v.get_attr(key).map_err(|e| e.to_string())?;
    if a.is_undefined() || a.is_none() {
        return Ok(None);
    }
    let a = object(
        &a,
        &["name", "street", "number", "postcode", "city", "country"],
    )
    .map_err(|e| format!("{key}: {e}"))?;
    let field = |k: &str, max: usize, needed: bool| -> Result<String, String> {
        let s = text(a, k)?;
        let s = match s {
            Some(s) => s,
            None if needed => return Err(format!("{key}: {k} is required")),
            None => String::new(),
        };
        max_len(&format!("{key} {k}"), &s, max)?;
        one_line(&format!("{key} {k}"), &s)?;
        Ok(s)
    };
    let country = field("country", 2, true)?.to_ascii_uppercase();
    if country.len() != 2 || !country.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(format!(
            "{key}: country is a two-letter ISO code such as CH"
        ));
    }
    Ok(Some([
        "S".to_string(),
        field("name", 70, true)?,
        field("street", 70, false)?,
        field("number", 16, false)?,
        field("postcode", 16, true)?,
        field("city", 35, true)?,
        country,
    ]))
}

fn swiss(v: &Value) -> Result<String, String> {
    let v = object(
        v,
        &[
            "account",
            "creditor",
            "amount",
            "currency",
            "debtor",
            "reference",
            "message",
            "billing",
            "alt",
        ],
    )?;
    let account = iban(&required(v, "account")?)?;
    if !(account.starts_with("CH") || account.starts_with("LI")) || account.len() != 21 {
        return Err("account is a Swiss or Liechtenstein IBAN, CH or LI and 21 characters".into());
    }
    let qr_iban = account[4..9]
        .parse::<u32>()
        .is_ok_and(|iid| (30000..=31999).contains(&iid));
    let creditor = swiss_address(v, "creditor")?.ok_or("creditor is required")?;
    let amount = amount(v)?.unwrap_or_default();
    let currency = match text(v, "currency")?.map(|c| c.to_ascii_uppercase()) {
        None => "CHF".to_string(),
        Some(c) if c == "CHF" || c == "EUR" => c,
        Some(c) => return Err(format!("currency is CHF or EUR, not {c}")),
    };
    let debtor = swiss_address(v, "debtor")?;
    let reference = text(v, "reference")?.map(|r| r.replace(' ', ""));
    let (kind, reference) = match reference {
        None if qr_iban => {
            return Err("a QR-IBAN needs a QR reference of 27 digits".into());
        }
        None => ("NON", String::new()),
        Some(r) if qr_iban => {
            if r.len() != 27 || !r.chars().all(|c| c.is_ascii_digit()) {
                return Err("a QR-IBAN takes a QR reference of 27 digits".into());
            }
            if mod10_recursive(&r[..26]).to_string() != r[26..] {
                return Err(format!(
                    "the QR reference has a wrong check digit: it should end in {}",
                    mod10_recursive(&r[..26])
                ));
            }
            ("QRR", r)
        }
        Some(r) if r.to_ascii_uppercase().starts_with("RF") => ("SCOR", creditor_reference(&r)?),
        Some(_) => {
            return Err(
                "with an ordinary IBAN the reference is a creditor reference (RF…); a QR \
                 reference needs a QR-IBAN"
                    .into(),
            )
        }
    };
    let message = text(v, "message")?.unwrap_or_default();
    one_line("message", &message)?;
    let billing = text(v, "billing")?.unwrap_or_default();
    one_line("billing", &billing)?;
    if message.chars().count() + billing.chars().count() > 140 {
        return Err("message and billing hold at most 140 characters together".into());
    }
    let alt = texts(v, "alt")?;
    if alt.len() > 2 {
        return Err("alt holds at most two alternative schemes".into());
    }
    for a in &alt {
        max_len("alt", a, 100)?;
        one_line("alt", a)?;
    }
    let mut lines: Vec<String> = vec!["SPC".into(), "0200".into(), "1".into(), account];
    lines.extend(creditor);
    lines.extend(std::iter::repeat_n(String::new(), 7));
    lines.push(amount);
    lines.push(currency);
    match debtor {
        Some(d) => lines.extend(d),
        None => lines.extend(std::iter::repeat_n(String::new(), 7)),
    }
    lines.push(kind.into());
    lines.push(reference);
    lines.push(message);
    lines.push("EPD".into());
    if !billing.is_empty() || !alt.is_empty() {
        lines.push(billing);
    }
    lines.extend(alt);
    let out = lines.join("\n");
    if out.chars().count() > 997 {
        return Err("the payload holds more than 997 characters".into());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// GS1 Digital Link.

/// Primary keys and the qualifiers that may follow each in the path, in
/// order (GS1 Digital Link URI syntax 1.2).
const GS1_KEYS: &[(&str, &[&str])] = &[
    ("01", &["22", "10", "21"]),
    ("00", &[]),
    ("253", &[]),
    ("255", &[]),
    ("401", &[]),
    ("402", &[]),
    ("414", &["254", "7040"]),
    ("417", &["7040"]),
    ("8003", &[]),
    ("8004", &["7040"]),
    ("8006", &["22", "10", "21"]),
    ("8010", &["8011"]),
    ("8013", &[]),
    ("8017", &["8019"]),
    ("8018", &["8019"]),
];

fn gs1(v: &Value, domain: Option<&str>) -> Result<String, String> {
    let element = match v.kind() {
        ValueKind::String => v.to_string(),
        _ => return Err("takes an element string such as (01)09506000134352(10)ABC".into()),
    };
    let fields =
        crate::rich::gs1_fields(element.trim()).map_err(|e| e.replace("gs1-128", "gs1"))?;
    let domain = domain.unwrap_or("https://id.gs1.org").trim_end_matches('/');
    let checked = url::Url::parse(domain).map_err(|_| format!("domain {domain:?} is not a URL"))?;
    if !matches!(checked.scheme(), "https" | "http")
        || checked.query().is_some()
        || checked.fragment().is_some()
    {
        return Err(format!(
            "domain {domain:?} is an http or https address without ? or #"
        ));
    }
    let keys: Vec<_> = fields
        .iter()
        .filter(|(ai, _, _)| GS1_KEYS.iter().any(|(k, _)| k == ai))
        .collect();
    let (key_ai, key_data, _) = match keys.as_slice() {
        [one] => *one,
        [] => return Err("a Digital Link needs one primary key, such as (01) for a GTIN".into()),
        _ => return Err("a Digital Link has one primary key, not several".into()),
    };
    let qualifiers = GS1_KEYS
        .iter()
        .find(|(k, _)| k == key_ai)
        .map_or(&[][..], |(_, q)| *q);
    let mut out = format!("{domain}/{key_ai}/{}", percent(key_data));
    for q in qualifiers {
        if let Some((_, data, _)) = fields.iter().find(|(ai, _, _)| ai == q) {
            let _ = write!(out, "/{q}/{}", percent(data));
        }
    }
    let mut query = Vec::new();
    for (ai, data, _) in &fields {
        if ai == key_ai || qualifiers.contains(&ai.as_str()) {
            continue;
        }
        query.push(format!("{ai}={}", percent(data)));
    }
    if !query.is_empty() {
        let _ = write!(out, "?{}", query.join("&"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(kind: &str, v: serde_json::Value) -> Result<String, String> {
        let value = Value::from(minijinja::value::Serde(&v));
        match kind {
            "wifi" => wifi(&value),
            "vcard" => vcard(&value),
            "mecard" => mecard(&value),
            "email" => email(&value),
            "tel" => tel(&value),
            "sms" => sms(&value),
            "geo" => geo(&value),
            "event" => event(&value),
            "epc" => epc(&value),
            "swiss" => swiss(&value),
            "gs1" => gs1(&value, None),
            _ => unreachable!(),
        }
    }

    #[test]
    fn wifi_escapes_and_checks() {
        assert_eq!(
            run("wifi", json!({"ssid": "Café;1", "password": "a\"b:c\\d,e"})).unwrap(),
            "WIFI:T:WPA;S:Café\\;1;P:a\\\"b\\:c\\\\d\\,e;;"
        );
        assert_eq!(
            run("wifi", json!({"ssid": "Guest", "hidden": true})).unwrap(),
            "WIFI:T:nopass;S:Guest;H:true;;"
        );
        assert!(run("wifi", json!({"ssid": "x", "security": "WPA"})).is_err());
        assert!(run("wifi", json!({"ssid": "x", "pasword": "y"}))
            .unwrap_err()
            .contains("unknown key \"pasword\""));
        assert!(run("wifi", json!({"password": "y"})).is_err());
    }

    #[test]
    fn contacts() {
        let card = run(
            "vcard",
            json!({"first": "Ada", "last": "Lovelace", "org": "Engines, Ltd",
                   "phone": ["+44 20 7946 0000", "07700 900000"], "email": "ada@example.com",
                   "address": {"street": "1 Main St", "city": "London", "country": "UK"},
                   "birthday": "1815-12-10", "note": "line one\nline two"}),
        )
        .unwrap();
        assert!(card
            .starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\nN:Lovelace;Ada;;;\r\nFN:Ada Lovelace\r\n"));
        assert!(card.contains("ORG:Engines\\, Ltd\r\n"));
        assert!(card.contains("TEL:+442079460000\r\nTEL:07700900000\r\n"));
        assert!(card.contains("ADR:;;1 Main St;London;;;UK\r\n"));
        assert!(card.contains("NOTE:line one\\nline two\r\n"));
        assert!(card.ends_with("END:VCARD"));
        assert!(run("vcard", json!({"org": "x"})).is_err());
        assert!(run("vcard", json!({"name": "x", "email": "nope"})).is_err());
        assert_eq!(
            run(
                "mecard",
                json!({"name": "Doe,John", "phone": "555-1234", "birthday": "1990-01-31"})
            )
            .unwrap(),
            "MECARD:N:Doe\\,John;TEL:5551234;BDAY:19900131;;"
        );
    }

    #[test]
    fn messages_calls_and_places() {
        assert_eq!(
            run("email", json!("a@example.com")).unwrap(),
            "mailto:a@example.com"
        );
        assert_eq!(
            run("email", json!({"to": ["a@example.com", "b@example.org"], "subject": "Order #12 & co", "body": "Hi\nthere"}))
                .unwrap(),
            "mailto:a@example.com,b@example.org?subject=Order%20%2312%20%26%20co&body=Hi%0D%0Athere"
        );
        assert_eq!(
            run("tel", json!("+1 (555) 010-9999")).unwrap(),
            "tel:+15550109999"
        );
        assert!(run("tel", json!("call me")).is_err());
        assert_eq!(
            run("sms", json!({"number": "+15550109999", "body": "STOP"})).unwrap(),
            "SMSTO:+15550109999:STOP"
        );
        assert_eq!(
            run(
                "geo",
                json!({"lat": 27.988056, "lon": 86.925278, "alt": 8848})
            )
            .unwrap(),
            "geo:27.988056,86.925278,8848"
        );
        assert!(run("geo", json!({"lat": 91, "lon": 0})).is_err());
    }

    #[test]
    fn events() {
        assert_eq!(
            run("event", json!({"title": "Launch; party", "start": "2026-12-01T18:00:00+01:00", "end": "2026-12-01T21:30:00+01:00", "location": "Hall 2"}))
                .unwrap(),
            "BEGIN:VEVENT\r\nSUMMARY:Launch\\; party\r\nDTSTART:20261201T170000Z\r\n\
             DTEND:20261201T203000Z\r\nLOCATION:Hall 2\r\nEND:VEVENT"
        );
        assert!(run("event", json!({"title": "Day", "start": "2026-12-01"}))
            .unwrap()
            .contains("DTSTART;VALUE=DATE:20261201\r\n"));
        assert!(run(
            "event",
            json!({"title": "x", "start": "2026-12-02", "end": "2026-12-01"})
        )
        .is_err());
        assert!(run("event", json!({"title": "x", "start": "tomorrow"})).is_err());
    }

    #[test]
    fn epc_follows_the_guidelines() {
        // The example from the EPC's guidelines, version 002 without a BIC.
        let out = run(
            "epc",
            json!({"name": "Red Cross of Belgium", "iban": "BE72 0000 0000 1616",
                   "amount": 1, "text": "Urgency fund"}),
        )
        .unwrap();
        assert_eq!(
            out,
            "BCD\n002\n1\nSCT\n\nRed Cross of Belgium\nBE72000000001616\nEUR1.00\n\n\nUrgency fund"
        );
        let out = run(
            "epc",
            json!({"name": "Acme", "iban": "DE89370400440532013000", "bic": "COBADEFFXXX",
                   "amount": "12.3", "purpose": "gdds", "reference": "RF18 5390 0754 7034"}),
        )
        .unwrap();
        assert_eq!(
            out,
            "BCD\n002\n1\nSCT\nCOBADEFFXXX\nAcme\nDE89370400440532013000\nEUR12.30\nGDDS\nRF18539007547034"
        );
        for bad in [
            json!({"name": "A", "iban": "DE89370400440532013001"}),
            json!({"name": "A", "iban": "DE89370400440532013000", "amount": 0}),
            json!({"name": "A", "iban": "DE89370400440532013000", "amount": 1.234}),
            json!({"name": "A", "iban": "DE89370400440532013000", "currency": "USD"}),
            json!({"name": "A", "iban": "DE89370400440532013000", "reference": "RF00 1", "text": "x"}),
            json!({"name": "A", "iban": "DE89370400440532013000", "reference": "RF19 5390 0754 7034"}),
            json!({"name": "A", "iban": "DE89370400440532013000", "bic": "XYZ"}),
        ] {
            assert!(run("epc", bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn swiss_qr_bill() {
        let creditor = json!({"name": "Robert Schneider AG", "street": "Rue du Lac",
                              "number": "1268", "postcode": "2501", "city": "Biel", "country": "CH"});
        // A QR-IBAN with a QR reference, from SIX's examples.
        let out = run(
            "swiss",
            json!({"account": "CH44 3199 9123 0008 8901 2", "creditor": creditor,
                   "amount": 1949.75, "reference": "21 00000 00003 13947 14300 09017",
                   "message": "Order of 15 June 2020", "billing": "//S1/10/10201409/11/200701/20/140.000-53/30/102673831/31/200615/32/7.7/33/7.7:139.40/40/0:30",
                   "debtor": {"name": "Pia-Maria Rutschmann-Schnyder", "street": "Grosse Marktgasse",
                              "number": "28", "postcode": "9400", "city": "Rorschach", "country": "CH"}}),
        )
        .unwrap();
        let lines: Vec<&str> = out.split('\n').collect();
        assert_eq!(&lines[..4], ["SPC", "0200", "1", "CH4431999123000889012"]);
        assert_eq!(lines[4], "S");
        assert_eq!(lines[18], "1949.75");
        assert_eq!(lines[19], "CHF");
        assert_eq!(lines[27], "QRR");
        assert_eq!(lines[28], "210000000003139471430009017");
        assert_eq!(lines[30], "EPD");
        assert_eq!(lines.len(), 32);
        // An ordinary IBAN takes a creditor reference, or none.
        let out = run(
            "swiss",
            json!({"account": "CH93 0076 2011 6238 5295 7", "creditor": creditor,
                   "currency": "EUR"}),
        )
        .unwrap();
        // No payload in the message: test output is no place for one.
        assert!(out.ends_with("\nNON\n\n\nEPD"), "no reference, then EPD");
        for bad in [
            json!({"account": "CH44 3199 9123 0008 8901 2", "creditor": creditor}),
            json!({"account": "CH93 0076 2011 6238 5295 7", "creditor": creditor,
                   "reference": "210000000003139471430009017"}),
            json!({"account": "CH44 3199 9123 0008 8901 2", "creditor": creditor,
                   "reference": "210000000003139471430009018"}),
            json!({"account": "DE89370400440532013000", "creditor": creditor}),
            json!({"account": "CH93 0076 2011 6238 5295 7"}),
            json!({"account": "CH93 0076 2011 6238 5295 7", "creditor": {"name": "x"}}),
            json!({"account": "CH93 0076 2011 6238 5295 7", "creditor": creditor, "currency": "USD"}),
        ] {
            assert!(run("swiss", bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn gs1_digital_link() {
        assert_eq!(
            run("gs1", json!("(01)09506000134352(17)261231(10)AB/12(21)X 1")).unwrap_err(),
            "(21) holds ' ', which GS1 does not allow"
        );
        assert_eq!(
            run("gs1", json!("(01)09506000134352(17)261231(10)AB/12(21)X1")).unwrap(),
            "https://id.gs1.org/01/09506000134352/10/AB%2F12/21/X1?17=261231"
        );
        assert_eq!(
            gs1(
                &Value::from("(414)9520123456788(254)32a"),
                Some("https://example.com/")
            )
            .unwrap(),
            "https://example.com/414/9520123456788/254/32a"
        );
        assert!(run("gs1", json!("(10)ABC")).is_err());
        assert!(run("gs1", json!("(01)09506000134353")).is_err());
        assert!(gs1(&Value::from("(01)09506000134352"), Some("javascript:x")).is_err());
    }

    #[test]
    fn check_digits() {
        assert_eq!(mod10_recursive("21000000000313947143000901"), 7);
        assert!(creditor_reference("RF18 5390 0754 7034").is_ok());
        assert!(iban("GB82 WEST 1234 5698 7654 32").is_ok());
        assert!(iban("GB82 WEST 1234 5698 7654 33").is_err());
    }

    #[test]
    fn text_that_is_not_ascii_is_refused_not_a_panic() {
        // Byte lengths that pass, with a character across the slice point.
        for bic in ["ABCDE\u{e9}XX", "ABCDEF\u{e9}X", "\u{e9}\u{e9}\u{e9}\u{e9}"] {
            let v = json!({"name": "A", "iban": "DE89370400440532013000", "bic": bic});
            assert!(run("epc", v).is_err(), "{bic}");
        }
        assert!(creditor_reference("RF1\u{e9}23").is_err());
        assert!(creditor_reference("RF\u{e9}1234").is_err());
        let v = json!({"name": "A", "iban": "DE89370400440532013000", "purpose": "\u{e9}\u{e9}"});
        assert!(run("epc", v).is_err());
    }

    #[test]
    fn an_address_cannot_add_to_a_mailto() {
        for to in [
            "a@example.com?bcc=evil@example.org",
            "a@example.com&body=x",
            "a%40b@example.com",
            "a@example.com#x",
            "a@example.com/x",
            "a@b@example.com",
        ] {
            assert!(run("email", json!(to)).is_err(), "{to}");
            assert!(run("email", json!({"to": to})).is_err(), "{to}");
            assert!(
                run("email", json!({"to": "ok@example.com", "cc": to})).is_err(),
                "{to}"
            );
        }
        assert_eq!(
            run("email", json!("first.last+tag@example.co.uk")).unwrap(),
            "mailto:first.last+tag@example.co.uk"
        );
    }
}
