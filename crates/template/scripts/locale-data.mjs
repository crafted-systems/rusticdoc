#!/usr/bin/env node
// Writes src/locale_data.rs, the CLDR data behind `number`, `currency` and
// `date` when a language is set, and tests/fixtures/cldr-vectors.json, the
// outputs the Rust code has to reproduce exactly.
//
// Numbers and currencies come from this Node's Intl, which is ICU with its CLDR
// data; month and day names come from the cldr-dates-full package of the same
// CLDR version, because Intl only exposes the names its own patterns use (a
// numeric month in Japanese, for one).
//
//   mkdir /tmp/cldr && (cd /tmp/cldr && npm i cldr-dates-full@48.2.0)
//   node crates/template/scripts/locale-data.mjs /tmp/cldr/node_modules/cldr-dates-full
//
// Run it with a Node whose `process.versions.cldr` matches the package.
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const crate = join(dirname(fileURLToPath(import.meta.url)), '..');
const datesDir = process.argv[2];
if (!datesDir) {
  console.error('usage: locale-data.mjs <path to cldr-dates-full>');
  process.exit(2);
}
const cldr = process.versions.cldr;
const pkg = JSON.parse(readFileSync(join(datesDir, 'package.json'), 'utf8')).version;
if (pkg.split('.')[0] !== cldr.split('.')[0]) {
  console.error(`Node has CLDR ${cldr} but cldr-dates-full is ${pkg}; use matching versions`);
  process.exit(2);
}

// Languages and regions with data. A tag not here falls back to its parent
// (`fr-lu` to `fr`), and a language not here keeps the plain formatting.
const LOCALES = [
  'en', 'en-us', 'en-gb', 'en-au', 'en-ca', 'en-ie', 'en-in', 'en-nz', 'en-za',
  'fr', 'fr-be', 'fr-ca', 'fr-ch', 'de', 'de-at', 'de-ch', 'de-li',
  'es', 'es-419', 'es-mx', 'es-us', 'it', 'it-ch', 'pt', 'pt-br', 'pt-pt',
  'nl', 'nl-be', 'sv', 'sv-fi', 'da', 'nb', 'nn', 'no', 'fi', 'is',
  'pl', 'cs', 'sk', 'hu', 'ro', 'bg', 'hr', 'sl', 'sr', 'uk', 'ru', 'el',
  'et', 'lv', 'lt', 'tr', 'ar', 'he', 'hi', 'th', 'id', 'ms', 'vi',
  'ja', 'zh', 'zh-tw', 'zh-hk', 'ko', 'ca', 'eu', 'gl',
];
// The cldr-json directory for a tag whose name there is not just its case.
const DATES_DIR = { 'zh-tw': 'zh-Hant', 'zh-hk': 'zh-Hant-HK' };

const intlTag = (t) => t.split('-').map((p, i) => (i && p.length === 2 ? p.toUpperCase() : p)).join('-');
const parent = (t) => (t.includes('-') ? t.slice(0, t.lastIndexOf('-')) : null);

/** A pattern for formatToParts output: `#` for the digits, `¤` for the symbol. */
function pattern(parts) {
  let out = '';
  for (const { type, value } of parts) {
    if (['integer', 'group', 'decimal', 'fraction'].includes(type)) {
      if (!out.endsWith('#')) out += '#';
    } else if (type === 'currency') {
      out += '¤';
    } else {
      out += value;
    }
  }
  return out;
}

function numberData(tag) {
  const l = intlTag(tag);
  const nf = new Intl.NumberFormat(l, { maximumFractionDigits: 3 });
  const parts = nf.formatToParts(1234567.891);
  const decimal = parts.find((p) => p.type === 'decimal').value;
  const group = parts.find((p) => p.type === 'group').value;
  const digits = nf.formatToParts(123456789).filter((p) => p.type === 'integer').map((p) => p.value.length);
  const primary = digits[digits.length - 1];
  const secondary = digits.length > 2 ? digits[digits.length - 2] : primary;
  const minGrouping = nf.formatToParts(1234).some((p) => p.type === 'group') ? 1 : 2;
  for (const p of parts) {
    if (p.type === 'integer' && !/^[0-9]+$/.test(p.value)) throw new Error(`${tag}: digits are not 0-9`);
  }
  const number = [pattern(nf.formatToParts(1234.5)), pattern(nf.formatToParts(-1234.5))];
  const money = (code, v) => pattern(new Intl.NumberFormat(l, { style: 'currency', currency: code }).formatToParts(v));
  // A currency whose symbol is not a letter next to the digits, so the pattern
  // carries no spacing for one.
  const symbolic = ['EUR', 'USD', 'GBP', 'JPY', ...CURRENCIES].find((code) => {
    const s = currencySymbol(l, code);
    const pat = money(code, 1);
    const near = pat.indexOf('¤') < pat.indexOf('#') ? s[s.length - 1] : s[0];
    return !/\p{L}/u.test(near);
  });
  const letters = [money('XTS', 1500), money('XTS', -1500)];
  // Romanian writes every currency as its code: then there is nothing else.
  const symbol = symbolic ? [money(symbolic, 1500), money(symbolic, -1500)] : letters;
  // A symbol whose CLDR text ends in a direction mark (Arabic `د.ك.‏`) comes
  // out of ICU as the symbol and a literal; put the mark back on the symbol.
  const fullSymbol = (code) => {
    let s = currencySymbol(l, code);
    const pat = money(code, 1500);
    for (const t of [symbol[0], letters[0]]) {
      if (pat !== t && pat.startsWith(t) && t.endsWith('¤')) s += pat.slice(t.length);
      else if (pat !== t && pat.endsWith(t) && t.startsWith('¤')) s = pat.slice(0, pat.length - t.length) + s;
      else continue;
      break;
    }
    return s;
  };
  const symbols = CURRENCIES.map((code) => [code, fullSymbol(code)]).filter(([c, s]) => c !== s);
  // Money may group differently from plain numbers: Austria writes
  // `1 234,5` but `€ 1.234,50`.
  const mparts = new Intl.NumberFormat(l, { style: 'currency', currency: 'EUR' }).formatToParts(1234567.891);
  const moneyDecimal = mparts.find((p) => p.type === 'decimal').value;
  const moneyGroup = mparts.find((p) => p.type === 'group').value;
  return { decimal, group, moneyDecimal, moneyGroup, primary, secondary, minGrouping, number, symbol, letters, symbols };
}

function currencySymbol(l, code) {
  return new Intl.NumberFormat(l, { style: 'currency', currency: code })
    .formatToParts(1)
    .find((p) => p.type === 'currency').value;
}

function dateNames(tag) {
  // A region CLDR does not list (en-US) is its language there.
  let dir = DATES_DIR[tag] ?? intlTag(tag);
  while (!existsSync(join(datesDir, 'main', dir)) && dir.includes('-')) dir = dir.slice(0, dir.lastIndexOf('-'));
  const file = join(datesDir, 'main', dir, 'ca-gregorian.json');
  const json = JSON.parse(readFileSync(file, 'utf8'));
  const g = json.main[dir].dates.calendars.gregorian;
  const months = (ctx, width) => Array.from({ length: 12 }, (_, i) => g.months[ctx][width][String(i + 1)]);
  // CLDR keys weekdays by name; Monday first, as chrono numbers them.
  const keys = ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'];
  const days = (width) => keys.map((k) => g.days.format[width][k]);
  return {
    months: [months('format', 'wide'), months('format', 'abbreviated'), months('stand-alone', 'wide'), months('stand-alone', 'abbreviated')],
    weekdays: [days('wide'), days('abbreviated')],
    amPm: [g.dayPeriods.format.abbreviated.am, g.dayPeriods.format.abbreviated.pm],
  };
}

const CURRENCIES = Intl.supportedValuesOf('currency');
const DIGITS = CURRENCIES.map((c) => [c, new Intl.NumberFormat('en', { style: 'currency', currency: c }).resolvedOptions().maximumFractionDigits])
  .filter(([, d]) => d !== 2);

const records = new Map();
for (const tag of LOCALES) records.set(tag, { number: numberData(tag), dates: dateNames(tag) });
// A region the same as its language adds nothing: lookups fall back to it.
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const kept = LOCALES.filter((tag) => {
  for (let p = parent(tag); p; p = parent(p)) {
    if (records.has(p)) return !same(records.get(p), records.get(tag));
  }
  return true;
});

const rs = (s) => JSON.stringify(s);
const list = (a) => `[${a.map(rs).join(', ')}]`;
const names = new Map();
let out = `// @generated by scripts/locale-data.mjs from CLDR ${cldr}; do not edit.
//! Number, currency and date-name data for each language with locale
//! formatting, from CLDR ${cldr}. Regenerate with the script; the tests compare
//! the output with tests/fixtures/cldr-vectors.json, written at the same time.

use crate::locale::{DateNames, LocaleData, NumberData};

/// The CLDR version this data comes from.
pub const CLDR_VERSION: &str = ${rs(cldr)};

/// Decimals for currencies that do not use two (ISO 4217, as CLDR has it).
pub static CURRENCY_DIGITS: &[(&str, u8)] = &[${DIGITS.map(([c, d]) => `(${rs(c)}, ${d})`).join(', ')}];
`;
for (const tag of kept) {
  const d = records.get(tag).dates;
  const key = JSON.stringify(d);
  if (names.has(key)) continue;
  const id = `NAMES_${tag.toUpperCase().replace(/-/g, '_')}`;
  names.set(key, id);
  out += `
static ${id}: DateNames = DateNames {
    months: [
${d.months.map((m) => `        ${list(m)},`).join('\n')}
    ],
    weekdays: [
${d.weekdays.map((m) => `        ${list(m)},`).join('\n')}
    ],
    am_pm: ${list(d.amPm)},
};
`;
}
out += `
/// Every language and region with data, sorted by tag.
pub static LOCALES: &[LocaleData] = &[
`;
for (const tag of [...kept].sort()) {
  const { number: n, dates } = records.get(tag);
  out += `    LocaleData {
        tag: ${rs(tag)},
        number: NumberData {
            decimal: ${rs(n.decimal)},
            group: ${rs(n.group)},
            money_decimal: ${rs(n.moneyDecimal)},
            money_group: ${rs(n.moneyGroup)},
            primary: ${n.primary},
            secondary: ${n.secondary},
            min_grouping: ${n.minGrouping},
            number: ${list(n.number)},
            symbol: ${list(n.symbol)},
            letters: ${list(n.letters)},
            symbols: &[${n.symbols.map(([c, s]) => `(${rs(c)}, ${rs(s)})`).join(', ')}],
        },
        dates: &${names.get(JSON.stringify(dates))},
    },
`;
}
out += '];\n';
writeFileSync(join(crate, 'src/locale_data.rs'), out);

// What Intl prints, for the Rust tests to match.
const numbers = [0, 1, 12, 999.9995, 1234, 1234.5, -1234.5, 12345.678, 1234567.891, 0.125, -0.5];
const moneyCodes = ['EUR', 'USD', 'GBP', 'JPY', 'KWD', 'CHF', 'INR', 'XTS'];
const amounts = [1500, -1500, 1234567.891, 0.5];
const vectors = { cldr, locales: {} };
for (const tag of LOCALES) {
  const l = intlTag(tag);
  const plain = new Intl.NumberFormat(l, { maximumFractionDigits: 3 });
  const two = new Intl.NumberFormat(l, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  const zero = new Intl.NumberFormat(l, { maximumFractionDigits: 0 });
  vectors.locales[tag] = {
    number: numbers.map((v) => [v, plain.format(v), two.format(v), zero.format(v)]),
    currency: moneyCodes.flatMap((code) => {
      const f = new Intl.NumberFormat(l, { style: 'currency', currency: code });
      return amounts.map((v) => [code, v, f.format(v)]);
    }),
  };
}
// One language per line, so a change of data reads as a change of lines.
const lines = Object.entries(vectors.locales).map(([tag, v]) => `  ${JSON.stringify(tag)}: ${JSON.stringify(v)}`);
writeFileSync(
  join(crate, 'tests/fixtures/cldr-vectors.json'),
  `{"cldr": ${JSON.stringify(cldr)}, "locales": {\n${lines.join(',\n')}\n}}\n`,
);
console.log(`${kept.length} of ${LOCALES.length} locales written, ${names.size} sets of names, CLDR ${cldr}`);
