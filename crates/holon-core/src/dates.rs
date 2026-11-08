//! The date leaves: JavaScript's `Date` in UTC (docs/engineering/themes/js-library-races/records.md,
//! "Dates"). Each was a native candidate in the races (`crates/holon-author/benches/races_records.rs`).
//!
//! A date is its time value, as in JavaScript: a whole number of milliseconds since
//! 1970-01-01T00:00:00Z, within ±8.64e15 (ECMA-262 §21.4.1.1), so `now` reaches these leaves as
//! `now * 1000`. Every leaf takes its date as `new Date(x)` does, a number through TimeClip
//! (`new Date(1.5)` is 1 ms) or a text through `Date.parse`, so `new Date(x).getUTCHours()` is
//! `date_part {part: hours}` of `x` whichever it is; and each gives nothing beyond the range, where
//! JavaScript's date is invalid (NaN). |t| ≤ 8.64e15 < 2⁵³, so a time value is exact as an `f64`
//! and as an `i64`, and every part is computed in `i64` on the proleptic Gregorian calendar (H.
//! Hinnant's `days_from_civil` and `civil_from_days`). MakeDay and MakeTime add and multiply in IEEE
//! arithmetic in the standard's order, which is correctly rounded, so every leaf gives the same bits
//! natively and in wasm; no `libm` function is used.
//!
//! Holon has no time zone (§12, the environment): there is no local time, only UTC and fixed
//! offsets. A leaf that reads a calendar (`date_part`, `date_text`, `date_add`) takes an offset, a
//! date's parts read at it; the business-day leaves read calendar dates, a bank's closing days
//! given as data (host-capabilities L4 §5.1.3).

use std::collections::BTreeSet;

use crate::Value;

const MS_PER_DAY: i64 = 86_400_000;
const MAX_TIME: f64 = 8.64e15;

/// `date_part`'s parts, its family: the time value itself (TimeClip: `new Date(ms)`), then each
/// `getUTC…`. `month` counts from 0, `day` is the weekday (0 is Sunday).
pub(crate) const PARTS: &[&str] = &["time", "year", "month", "date", "day", "hours", "minutes", "seconds", "milliseconds"];

/// `date_set`'s parts, its family: the first part a `setUTC…` replaces, in MakeDate's order.
pub(crate) const SET_PARTS: &[&str] = &["year", "month", "date", "hours", "minutes", "seconds", "milliseconds"];

/// `date_text`'s formats: `toISOString`, `toUTCString`, and the date alone (`YYYY-MM-DD`).
pub(crate) const FORMATS: &[&str] = &["iso", "utc", "date"];

/// `date_add`'s units, its family.
pub(crate) const UNITS: &[&str] = &["days", "months", "years"];

/// The business-day leaves' weekends, their family: Saturday and Sunday, Friday and Saturday, or
/// none (a calendar whose every closing day is a holiday).
pub(crate) const WEEKENDS: &[&str] = &["sat_sun", "fri_sat", "none"];

/// The days a time value falls on, either side of 1970-01-01: ±8.64e15 ms is ±1e8 days.
const MAX_DAY: i64 = 100_000_000;

/// The most business days `business_day_add` steps, either way.
const MAX_BUSINESS_DAYS: f64 = 100_000.0;

/// ToIntegerOrInfinity on a finite number: truncated, `-0` made `0`.
fn to_integer(x: f64) -> f64 {
    x.trunc() + 0.0
}

/// TimeClip: nothing beyond ±8.64e15 or not finite, else the whole number, `-0` made `0`.
fn clip(tv: f64) -> Option<f64> {
    (tv.is_finite() && tv.abs() <= MAX_TIME).then(|| to_integer(tv))
}

/// The time value a leaf takes, as `new Date(x)` makes it: a number through TimeClip, a text as
/// `Date.parse` reads it.
fn time_value(x: &Value) -> Option<i64> {
    match x {
        Value::Number(t) => clip(*t).map(|t| t as i64),
        Value::Text(s) => parse(s).map(|t| t as i64),
        _ => None,
    }
}

/// An offset in minutes east of UTC: `"Z"`, or `"±HH:mm"` (`HH` up to 23, `mm` up to 59) as ISO
/// 8601 and `Date.parse` write one, or a whole number of minutes from −1439 to 1439.
fn offset_minutes(x: &Value) -> Option<i64> {
    match x {
        Value::Number(m) if m.fract() == 0.0 && m.abs() <= 1439.0 => Some(*m as i64),
        Value::Text(s) if s == "Z" => Some(0),
        Value::Text(s) => {
            let &[sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] = s.as_bytes() else { return None };
            if ![h1, h2, m1, m2].iter().all(u8::is_ascii_digit) {
                return None;
            }
            let (h, m) = (i64::from((h1 - b'0') * 10 + (h2 - b'0')), i64::from((m1 - b'0') * 10 + (m2 - b'0')));
            (h <= 23 && m <= 59).then_some(if sign == b'+' { h * 60 + m } else { -(h * 60 + m) })
        }
        _ => None,
    }
}

/// The offset a leaf is given, in milliseconds: none is UTC's; one that isn't an offset, nothing.
fn offset_ms(offset: Option<&Value>) -> Option<i64> {
    offset.map_or(Some(0), |o| offset_minutes(o).map(|m| m * 60_000))
}

/// An offset as ISO 8601 writes it after a time: `Z` for none, else `±HH:mm`.
fn offset_text(ms: i64) -> String {
    if ms == 0 {
        return "Z".into();
    }
    let m = ms.abs() / 60_000;
    format!("{}{:02}:{:02}", if ms < 0 { '-' } else { '+' }, m / 60, m % 60)
}

/// A year as `toISOString` writes it: four digits from 0 to 9999, else a sign and six.
fn iso_year(y: i64) -> String {
    match y {
        0..=9999 => format!("{y:04}"),
        _ if y < 0 => format!("-{:06}", -y),
        _ => format!("+{y:06}"),
    }
}

/// Days since 1970-01-01 of a civil date, month 1 to 12.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * if m > 2 { m - 3 } else { m + 9 } + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count since 1970-01-01: year, month 1 to 12, day 1 to 31.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

/// The UTC parts of a time value: year, month (0 to 11, as JavaScript counts), date, weekday (0 is
/// Sunday), hours, minutes, seconds, milliseconds.
fn parts(t: i64) -> [i64; 8] {
    let day = t.div_euclid(MS_PER_DAY);
    let ms = t.rem_euclid(MS_PER_DAY);
    let (y, m, d) = civil_from_days(day);
    [y, m - 1, d, (day + 4).rem_euclid(7), ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000]
}

/// MakeDay, ECMA-262 §21.4.1.28, with V8's bounds: the year (truncated) within ±1,000,000 and the
/// month within ±10,000,000, the month carried into the year, then the day count of the month's
/// first day plus `date - 1` in IEEE arithmetic, as the standard writes `Day(t) + dt − 1𝔽`.
fn make_day(year: f64, month: f64, date: f64) -> Option<f64> {
    if !(year.is_finite() && month.is_finite() && date.is_finite()) {
        return None;
    }
    let (y, m) = (to_integer(year), to_integer(month));
    if !(-1e6..=1e6).contains(&y) || !(-1e7..=1e7).contains(&m) {
        return None;
    }
    let (y, m) = (y as i64 + (m as i64).div_euclid(12), (m as i64).rem_euclid(12));
    Some(days_from_civil(y, m + 1, 1) as f64 + to_integer(date) - 1.0)
}

/// MakeTime, §21.4.1.27: `((h * 3600000 + m * 60000) + s * 1000) + ms`, each truncated first, in
/// IEEE arithmetic and that order: for huge parts the order decides the last bit.
fn make_time(h: f64, m: f64, s: f64, ms: f64) -> Option<f64> {
    [h, m, s, ms].iter().all(|x| x.is_finite()).then(|| to_integer(h) * 3_600_000.0 + to_integer(m) * 60_000.0 + to_integer(s) * 1000.0 + to_integer(ms))
}

/// TimeClip(MakeDate(MakeDay(year, month, date), MakeTime(h, m, s, ms))), MakeDate being
/// `day * 86400000 + time`: what `Date.UTC` computes after its year mapping, and every `setUTC…`
/// with the parts it isn't given read from the date.
fn utc_time([y, mo, d, h, mi, s, ms]: [f64; 7]) -> Option<f64> {
    clip(make_day(y, mo, d)? * 86_400_000.0 + make_time(h, mi, s, ms)?)
}

/// The numbers of `inputs`, or nothing when one isn't a number.
fn numbers(inputs: &[&Value]) -> Option<Vec<f64>> {
    inputs.iter().map(|x| if let Value::Number(n) = x { Some(*n) } else { None }).collect()
}

/// `date_part {part}`: `new Date(x)`'s time value (`time`: TimeClip of a number, `Date.parse` of a
/// text), or one UTC part of it, as each `getUTC…` gives it; given an offset, the part there. The
/// time value is the instant, the same at any offset.
pub(crate) fn part(x: &Value, offset: Option<&Value>, part: &str) -> Option<f64> {
    let t = time_value(x)?;
    let shift = offset_ms(offset)?;
    let k = PARTS.iter().position(|p| *p == part)?;
    Some(if k == 0 { t as f64 } else { parts(t + shift)[k - 1] as f64 })
}

/// `date_utc`, `Date.UTC(y, m, d, h, mi, s, ms)`: 1 to 7 numbers, the missing parts month 0, date
/// 1 and the rest 0; a year from 0 to 99 after truncation is 1900 plus it (`Date.UTC(99, 0)` is
/// 1999). With no number, nothing (JavaScript's NaN).
pub(crate) fn utc(inputs: &[&Value]) -> Option<f64> {
    let given = numbers(inputs)?;
    if given.is_empty() || given.len() > 7 {
        return None;
    }
    let mut args = [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
    args[..given.len()].copy_from_slice(&given);
    if args[0].is_finite() {
        let y = to_integer(args[0]);
        if (0.0..=99.0).contains(&y) {
            args[0] = 1900.0 + y;
        }
    }
    utc_time(args)
}

/// How many numbers `date_set {part}` takes after the date: `setUTCFullYear(y, m?, d?)`,
/// `setUTCMonth(m, d?)`, `setUTCDate(d)`, `setUTCHours(h, m?, s?, ms?)`, `setUTCMinutes(m, s?,
/// ms?)`, `setUTCSeconds(s, ms?)`, `setUTCMilliseconds(ms)`.
pub(crate) fn set_takes(part: &str) -> Option<usize> {
    Some(match part {
        "year" | "minutes" => 3,
        "month" | "seconds" => 2,
        "date" | "milliseconds" => 1,
        "hours" => 4,
        _ => return None,
    })
}

/// `date_set {part}`, `setUTC…` as the value it gives: the date with the parts given, starting at
/// `part`, in place of its own, through MakeDay, MakeTime, MakeDate and TimeClip, as JavaScript
/// computes it. No year mapping (`setUTCFullYear(99)` is year 99). More numbers than the setter
/// takes, or none, give nothing.
pub(crate) fn set(t: &Value, given: &[&Value], part: &str) -> Option<f64> {
    let t = time_value(t)?;
    let given = numbers(given)?;
    if given.is_empty() || given.len() > set_takes(part)? {
        return None;
    }
    let k = SET_PARTS.iter().position(|p| *p == part)?;
    let p = parts(t);
    let mut args = [p[0], p[1], p[2], p[4], p[5], p[6], p[7]].map(|n| n as f64);
    args[k..k + given.len()].copy_from_slice(&given);
    utc_time(args)
}

/// `date_text {format}`: `iso` is `toISOString` (`YYYY-MM-DDTHH:mm:ss.sssZ`, a year outside 0 to
/// 9999 as a sign and six digits); `date` its date alone; `utc` is `toUTCString` (`Thu, 01 Jan
/// 1970 00:00:00 GMT`, a negative year as `-` and at least four digits). Given an offset, `iso`
/// and `date` are the time there, `iso` written with it (`+01:00`; `Z` for a zero offset); `utc`
/// is GMT's, so nothing for another.
pub(crate) fn text(t: &Value, offset: Option<&Value>, format: &str) -> Option<String> {
    let shift = offset_ms(offset)?;
    let [y, mo, d, wd, h, mi, s, ms] = parts(time_value(t)? + shift);
    let date = format!("{}-{:02}-{d:02}", iso_year(y), mo + 1);
    Some(match format {
        "iso" => format!("{date}T{h:02}:{mi:02}:{s:02}.{ms:03}{}", offset_text(shift)),
        "date" => date,
        "utc" if shift == 0 => {
            let day = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][wd as usize];
            let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][mo as usize];
            let year = if y >= 0 { format!("{y:04}") } else { format!("-{:04}", -y) };
            format!("{day}, {d:02} {month} {year} {h:02}:{mi:02}:{s:02} GMT")
        }
        _ => return None,
    })
}

/// `date_add {unit}`: the time value `amount` days, months or years later (earlier when
/// negative), the amount truncated, at the offset's calendar. A day is 86,400,000 ms, at any
/// fixed offset; a month keeps the day of the month, or is the month's last day when it has
/// fewer (January 31 and a month is February 28, as date-fns's `addMonths`), and a year is twelve
/// months. The time of day is kept. Nothing past the range.
pub(crate) fn add(t: &Value, amount: &Value, offset: Option<&Value>, unit: &str) -> Option<f64> {
    let t = time_value(t)?;
    let shift = offset_ms(offset)?;
    let &Value::Number(n) = amount else { return None };
    if !n.is_finite() {
        return None;
    }
    let n = to_integer(n);
    let months = match unit {
        "days" if n.abs() <= 2.0 * MAX_DAY as f64 => return clip((t + n as i64 * MS_PER_DAY) as f64),
        "months" => n,
        "years" => n * 12.0,
        _ => return None,
    };
    if months.abs() > 1e7 {
        return None;
    }
    let local = t + shift;
    let (y, m, d) = civil_from_days(local.div_euclid(MS_PER_DAY));
    let total = y * 12 + (m - 1) + months as i64;
    let (y, m) = (total.div_euclid(12), total.rem_euclid(12) + 1);
    let day = days_from_civil(y, m, d.min(days_in_month(y, m)));
    clip((day * MS_PER_DAY + local.rem_euclid(MS_PER_DAY) - shift) as f64)
}

/// A calendar date, as its day since 1970-01-01: a `YYYY-MM-DD` text (`±YYYYYY-MM-DD` for a far
/// year), or a time value's date in UTC. A text with a time of day isn't one: its date depends on
/// an offset the leaf can't know.
fn calendar_day(x: &Value) -> Option<i64> {
    match x {
        Value::Number(_) => time_value(x).map(|t| t.div_euclid(MS_PER_DAY)),
        Value::Text(s) => {
            let b = s.as_bytes();
            let date = match b.len() {
                10 => b[0].is_ascii_digit() && b[4] == b'-' && b[7] == b'-',
                13 => matches!(b[0], b'+' | b'-') && b[7] == b'-' && b[10] == b'-',
                _ => false,
            };
            if !date {
                return None;
            }
            parse(s).map(|t| t as i64 / MS_PER_DAY)
        }
        _ => None,
    }
}

/// A calendar date as text, `YYYY-MM-DD`, as `toISOString` writes its date; nothing past the range.
fn calendar_text(day: i64) -> Option<String> {
    let (y, m, d) = civil_from_days(day);
    (day.abs() <= MAX_DAY).then(|| format!("{}-{m:02}-{d:02}", iso_year(y)))
}

/// The days a business-day leaf counts: those in neither its weekend nor its holidays.
struct Calendar {
    /// By weekday, 0 Sunday: whether it is in the weekend.
    weekend: [bool; 7],
    holidays: BTreeSet<i64>,
}

impl Calendar {
    /// The calendar a leaf is given: `holidays` a list of calendar dates, or none; nothing when an
    /// item isn't a date, as a malformed CSV is refused.
    fn new(holidays: Option<&Value>, weekend: &str) -> Option<Calendar> {
        let weekend = match weekend {
            "sat_sun" => [true, false, false, false, false, false, true],
            "fri_sat" => [false, false, false, false, false, true, true],
            "none" => [false; 7],
            _ => return None,
        };
        let holidays = match holidays {
            None => BTreeSet::new(),
            Some(Value::List(xs)) => xs.iter().map(calendar_day).collect::<Option<_>>()?,
            Some(_) => return None,
        };
        Some(Calendar { weekend, holidays })
    }

    fn weekend(&self, day: i64) -> bool {
        self.weekend[(day + 4).rem_euclid(7) as usize]
    }

    fn open(&self, day: i64) -> bool {
        !self.weekend(day) && !self.holidays.contains(&day)
    }

    /// How many business days `lo..=hi` holds: its whole weeks, the days left, less the holidays
    /// among them that aren't in the weekend, so not the range's length.
    fn open_in(&self, lo: i64, hi: i64) -> i64 {
        if hi < lo {
            return 0;
        }
        let n = hi - lo + 1;
        let per_week = self.weekend.iter().filter(|w| !**w).count() as i64;
        let rest = (0..n % 7).filter(|k| !self.weekend(lo + k)).count() as i64;
        let closed = self.holidays.range(lo..=hi).filter(|&&h| !self.weekend(h)).count() as i64;
        n / 7 * per_week + rest - closed
    }
}

/// `business_day_add {weekend}`: the calendar date `days` business days after `date` (before it
/// when negative), the count truncated, up to 100,000 either way; with 0, `date` when it is a
/// business day, else the next one. As text, `YYYY-MM-DD`: a date has no time and no offset.
pub(crate) fn business_day_add(date: &Value, days: &Value, holidays: Option<&Value>, weekend: &str) -> Option<String> {
    let mut day = calendar_day(date)?;
    let calendar = Calendar::new(holidays, weekend)?;
    let &Value::Number(n) = days else { return None };
    if !n.is_finite() || n.abs() > MAX_BUSINESS_DAYS {
        return None;
    }
    let n = to_integer(n) as i64;
    // At most |days| × 7 / 5 steps, the holidays and a week: the weekend has two days at most.
    let (step, mut left) = if n == 0 { (1, i64::from(!calendar.open(day))) } else { (n.signum(), n.abs()) };
    while left > 0 {
        day += step;
        if day.abs() > MAX_DAY {
            return None;
        }
        if calendar.open(day) {
            left -= 1;
        }
    }
    calendar_text(day)
}

/// `business_days_between {weekend}`: the business days counted from `from`, not itself, to `to`,
/// itself included: after `from` up to `to` when `to` is later, else from `to` up to `from`,
/// negated. So `business_days_between(d, business_day_add(d, n))` is `n` for any `n` but 0.
pub(crate) fn business_days_between(from: &Value, to: &Value, holidays: Option<&Value>, weekend: &str) -> Option<f64> {
    let (a, b) = (calendar_day(from)?, calendar_day(to)?);
    let calendar = Calendar::new(holidays, weekend)?;
    Some(if b >= a { calendar.open_in(a + 1, b) } else { -calendar.open_in(b, a - 1) } as f64)
}

/// `is_business_day {weekend}`: whether `date` is in neither the weekend nor the holidays.
pub(crate) fn is_business_day(date: &Value, holidays: Option<&Value>, weekend: &str) -> Option<bool> {
    let day = calendar_day(date)?;
    Some(Calendar::new(holidays, weekend)?.open(day))
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if y.rem_euclid(4) == 0 && (y.rem_euclid(100) != 0 || y.rem_euclid(400) == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `date_parse`, `Date.parse(s)`: ECMA-262's Date Time String Format (§21.4.1.32) exactly. `YYYY`,
/// `YYYY-MM` or `YYYY-MM-DD` (a year may be `±YYYYYY`, never `-000000`), then optionally `THH:mm`,
/// `:ss` and `.sss` (three digits), and `Z` or `±HH:mm`; `24:00` (all zeros after it) is the end
/// of the day. A date alone is UTC; a time without an offset is local time in JavaScript, and Holon
/// has no local time, so it gives nothing, as does anything outside the format (where an engine
/// falls back to heuristics of its own) or beyond ±8.64e15.
pub(crate) fn parse(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut i = 0usize;
    let digits = |i: &mut usize, n: usize| -> Option<i64> {
        let run = b.get(*i..*i + n)?;
        if !run.iter().all(u8::is_ascii_digit) {
            return None;
        }
        *i += n;
        Some(run.iter().fold(0, |a, d| a * 10 + i64::from(d - b'0')))
    };
    let year = match b.first()? {
        &c @ (b'+' | b'-') => {
            i = 1;
            let y = digits(&mut i, 6)?;
            if c == b'-' && y == 0 {
                return None;
            }
            if c == b'-' { -y } else { y }
        }
        _ => digits(&mut i, 4)?,
    };
    let (mut month, mut day) = (1, 1);
    if b.get(i) == Some(&b'-') {
        i += 1;
        month = digits(&mut i, 2)?;
        if b.get(i) == Some(&b'-') {
            i += 1;
            day = digits(&mut i, 2)?;
        }
    }
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    let date = days_from_civil(year, month, day) * MS_PER_DAY;
    if i == b.len() {
        return clip(date as f64);
    }
    if b.get(i) != Some(&b'T') {
        return None;
    }
    i += 1;
    let h = digits(&mut i, 2)?;
    if b.get(i) != Some(&b':') {
        return None;
    }
    i += 1;
    let mi = digits(&mut i, 2)?;
    let (mut sec, mut ms) = (0, 0);
    if b.get(i) == Some(&b':') {
        i += 1;
        sec = digits(&mut i, 2)?;
        if b.get(i) == Some(&b'.') {
            i += 1;
            ms = digits(&mut i, 3)?;
        }
    }
    if h > 24 || mi > 59 || sec > 59 || (h == 24 && (mi, sec, ms) != (0, 0, 0)) {
        return None;
    }
    let offset = match b.get(i) {
        Some(b'Z') => {
            i += 1;
            0
        }
        Some(&c @ (b'+' | b'-')) => {
            i += 1;
            let oh = digits(&mut i, 2)?;
            if b.get(i) != Some(&b':') {
                return None;
            }
            i += 1;
            let om = digits(&mut i, 2)?;
            if oh > 23 || om > 59 {
                return None;
            }
            let o = (oh * 60 + om) * 60_000;
            if c == b'+' { o } else { -o }
        }
        _ => return None,
    };
    if i != b.len() {
        return None;
    }
    clip((date + ((h * 60 + mi) * 60 + sec) * 1000 + ms - offset) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::compute;
    use crate::Param;

    fn t(x: f64) -> Value {
        Value::Number(x)
    }
    fn txt(s: &str) -> Value {
        Value::Text(s.into())
    }
    fn leaf(name: &str, params: &[(&str, &str)], inputs: &[Value]) -> Option<Value> {
        let params: Vec<Param> = params.iter().map(|&(k, v)| Param::frozen(k, v)).collect();
        compute(name, &inputs.iter().collect::<Vec<_>>(), &params)
    }
    fn iso(ms: f64) -> Option<Value> {
        leaf("date_text", &[("format", "iso")], &[t(ms)])
    }
    fn parsed(s: &str) -> Option<Value> {
        leaf("date_parse", &[], &[txt(s)])
    }

    /// What JavaScript gives for each (the bench's known answers, the standard's): toISOString and
    /// Date.parse both ways, toUTCString, getUTCDay.
    #[test]
    fn the_texts_and_the_parse_give_javascripts_known_answers() {
        for (ms, text) in [
            (0.0, "1970-01-01T00:00:00.000Z"),
            (-1.0, "1969-12-31T23:59:59.999Z"),
            (951_782_400_000.0, "2000-02-29T00:00:00.000Z"),
            (-62_135_596_800_000.0, "0001-01-01T00:00:00.000Z"),
            (-62_167_219_200_000.0, "0000-01-01T00:00:00.000Z"),
            (-62_198_755_200_000.0, "-000001-01-01T00:00:00.000Z"),
            (253_402_300_799_999.0, "9999-12-31T23:59:59.999Z"),
            (253_402_300_800_000.0, "+010000-01-01T00:00:00.000Z"),
            (8.64e15, "+275760-09-13T00:00:00.000Z"),
            (-8.64e15, "-271821-04-20T00:00:00.000Z"),
        ] {
            assert_eq!(iso(ms), Some(txt(text)), "toISOString of {ms}");
            assert_eq!(parsed(text), Some(t(ms)), "Date.parse({text})");
        }
        assert_eq!(iso(8.64e15 + 1.0), None);
        assert_eq!(iso(1.5), iso(1.0), "a time value is clipped first");
        assert_eq!(leaf("date_text", &[("format", "utc")], &[t(0.0)]), Some(txt("Thu, 01 Jan 1970 00:00:00 GMT")));
        assert_eq!(leaf("date_text", &[("format", "utc")], &[t(951_782_400_000.0)]), Some(txt("Tue, 29 Feb 2000 00:00:00 GMT")));
        assert_eq!(leaf("date_text", &[("format", "utc")], &[t(-62_198_755_200_000.0)]), Some(txt("Fri, 01 Jan -0001 00:00:00 GMT")));
        assert_eq!(leaf("date_text", &[("format", "local")], &[t(0.0)]), None);
        assert_eq!(leaf("date_part", &[("part", "day")], &[t(0.0)]), Some(t(4.0)));
        for (s, ms) in [
            ("2000", Some(946_684_800_000.0)),
            ("2000-02", Some(949_363_200_000.0)),
            ("2000-02-29T12:00:00Z", Some(951_825_600_000.0)),
            ("2000-02-29T12:00:00+01:00", Some(951_822_000_000.0)),
            ("2000-02-29T12:00Z", Some(951_825_600_000.0)),
            ("2000-02-29T24:00:00Z", Some(951_868_800_000.0)),
            ("2000-02-29T12:00:00.123-00:30", Some(951_825_600_123.0 + 1_800_000.0)),
            ("2000-02-29T12:00:00", None),
            ("2000-02-30", None),
            ("2001-02-29", None),
            ("2000-13-01", None),
            ("2000-02-29T24:00:01Z", None),
            ("-000000-01-01T00:00:00Z", None),
            ("+275760-09-13T00:00:00.001Z", None),
            ("2000-02-29T12:00:00.5Z", None),
            ("Tue, 29 Feb 2000 00:00:00 GMT", None),
            ("2000-2-29", None),
            ("", None),
        ] {
            assert_eq!(parsed(s), ms.map(t), "Date.parse({s:?})");
        }
    }

    /// `new Date(ms)` is TimeClip, `new Date(text)` Date.parse; each getUTC… part, over times at
    /// the edges of days, years, eras and the range.
    #[test]
    fn each_part_is_what_its_getter_gives() {
        for (x, want) in [(0.0, Some(0.0)), (-0.0, Some(0.0)), (1.5, Some(1.0)), (-1.5, Some(-1.0)), (-0.5, Some(0.0)), (8.64e15, Some(8.64e15)), (8_640_000_000_000_002.0, None), (-8.64e15, Some(-8.64e15)), (1e300, None)] {
            let got = leaf("date_part", &[("part", "time")], &[t(x)]);
            assert_eq!(got, want.map(t), "new Date({x})");
            if let Some(Value::Number(n)) = got {
                assert!(n != 0.0 || n.is_sign_positive(), "-0 is made 0");
            }
        }
        assert_eq!(leaf("date_part", &[("part", "time")], &[txt("2000-02-29")]), Some(t(951_782_400_000.0)), "new Date(text)");
        assert_eq!(leaf("date_part", &[("part", "year")], &[txt("2000-02-29")]), Some(t(2000.0)));
        assert_eq!(leaf("date_part", &[("part", "time")], &[txt("29 Feb")]), None);
        assert_eq!(leaf("date_part", &[("part", "time")], &[Value::Boolean(true)]), None);
        let names = ["year", "month", "date", "day", "hours", "minutes", "seconds", "milliseconds"];
        let at = |ms: f64| names.map(|p| leaf("date_part", &[("part", p)], &[t(ms)]));
        // 2000-02-29T23:59:59.999Z, a Tuesday.
        assert_eq!(at(951_868_799_999.0), [2000.0, 1.0, 29.0, 2.0, 23.0, 59.0, 59.0, 999.0].map(|x| Some(t(x))));
        assert_eq!(at(-1.0), [1969.0, 11.0, 31.0, 3.0, 23.0, 59.0, 59.0, 999.0].map(|x| Some(t(x))));
        assert_eq!(at(-62_198_755_200_000.0), [-1.0, 0.0, 1.0, 5.0, 0.0, 0.0, 0.0, 0.0].map(|x| Some(t(x))));
        assert_eq!(at(-8.64e15), [-271_821.0, 3.0, 20.0, 2.0, 0.0, 0.0, 0.0, 0.0].map(|x| Some(t(x))));
        assert_eq!(at(1_234_567_890_123.0), [2009.0, 1.0, 13.0, 5.0, 23.0, 31.0, 30.0, 123.0].map(|x| Some(t(x))));
        assert_eq!(leaf("date_part", &[("part", "week")], &[t(0.0)]), None);
        // Every part, over the bench's times, rebuilt by Date.UTC, is the time again (none of
        // them in a year from 0 to 99, which Date.UTC maps).
        for ms in [0.0, 1.0, -1.0, 999.0, 86_399_999.0, -86_400_001.0, 951_782_400_000.0, 1_709_251_199_999.0, -62_198_755_200_000.0, 253_402_300_800_000.0, 8.64e15, -8.64e15 + 1.0] {
            let [y, mo, d, _, h, mi, s, ms_] = at(ms).map(|v| v.unwrap());
            assert_eq!(leaf("date_utc", &[], &[y, mo, d, h, mi, s, ms_]), Some(t(ms)), "Date.UTC of the parts of {ms}");
        }
    }

    /// Date.UTC: the year mapping, a month or date past its end carried, the range's end, and
    /// MakeTime's order on huge parts.
    #[test]
    fn date_utc_is_make_date_of_its_parts() {
        let utc = |args: &[f64]| leaf("date_utc", &[], &args.iter().map(|&x| t(x)).collect::<Vec<_>>());
        let cases: [(&[f64], Option<f64>); 16] = [
            (&[2000.0, 1.0, 29.0, 0.0, 0.0, 0.0, 0.0], Some(951_782_400_000.0)),
            (&[99.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0], Some(915_148_800_000.0)),
            (&[2020.0, 12.0, 1.0, 0.0, 0.0, 0.0, 0.0], Some(1_609_459_200_000.0)),
            (&[2021.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], Some(1_609_372_800_000.0)),
            (&[275_760.0, 8.0, 13.0, 0.0, 0.0, 0.0, 0.0], Some(8.64e15)),
            (&[275_760.0, 8.0, 13.0, 0.0, 0.0, 0.0, 1.0], None),
            (&[1970.0, 0.0, 1.0, 25.0, 61.0, 61.0, 1001.0], Some(((25.0 * 60.0 + 61.0) * 60.0 + 61.0) * 1000.0 + 1001.0)),
            // Fewer parts: month 0, date 1, the rest 0.
            (&[2000.0], Some(946_684_800_000.0)),
            (&[2000.0, 1.0], Some(949_363_200_000.0)),
            (&[99.9], Some(915_148_800_000.0)),
            (&[100.0, 0.0], Some(-59_011_459_200_000.0)),
            (&[-0.5, 0.0], Some(-2_208_988_800_000.0)),
            (&[1e6 + 1.0, 0.0], None),
            (&[1970.0, 0.0, 1.0, 0.0, 0.0, 0.0, 8_640_000_000_000_001.0], None),
            (&[], None),
            (&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0], None),
        ];
        for (args, ms) in cases {
            assert_eq!(utc(args), ms.map(t), "Date.UTC{args:?}");
        }
        assert_eq!(leaf("date_utc", &[], &[txt("2000")]), None);
    }

    /// Each setUTC… replaces the parts it is given, from its own, and computes the rest from the
    /// date, as JavaScript does.
    #[test]
    fn date_set_replaces_the_parts_it_is_given() {
        let set = |part: &str, ms: f64, xs: &[f64]| {
            let mut inputs = vec![t(ms)];
            inputs.extend(xs.iter().map(|&x| t(x)));
            leaf("date_set", &[("part", part)], &inputs)
        };
        let year_of = |v: Option<Value>| v.and_then(|v| leaf("date_part", &[("part", "year")], &[v]));
        let leap = 951_782_400_000.0; // 2000-02-29T00:00:00.000Z
        assert_eq!(set("year", leap, &[2001.0]), Some(t(983_404_800_000.0)), "Feb 29 2001 is Mar 1");
        assert_eq!(year_of(set("year", leap, &[99.0])), Some(t(99.0)), "no year mapping");
        assert_eq!(set("year", leap, &[2000.0, 0.0, 15.0]), Some(t(947_894_400_000.0)));
        assert_eq!(set("month", leap, &[0.0]), Some(t(949_104_000_000.0)), "Jan 29");
        assert_eq!(set("month", leap, &[13.0, 1.0]), Some(t(980_985_600_000.0)), "month 13 is the next year's February: Feb 1 2001");
        assert_eq!(set("date", leap, &[0.0]), Some(t(949_276_800_000.0)), "date 0 is the day before the 1st: Jan 31");
        assert_eq!(set("hours", leap, &[25.0]), Some(t(leap + 25.0 * 3_600_000.0)));
        assert_eq!(set("hours", leap, &[1.0, 2.0, 3.0, 4.0]), Some(t(leap + 3_723_004.0)));
        assert_eq!(set("minutes", leap, &[61.0]), Some(t(leap + 61.0 * 60_000.0)));
        assert_eq!(set("seconds", leap, &[-0.5]), Some(t(leap)), "-0.5 is truncated to 0");
        assert_eq!(set("milliseconds", leap, &[1001.0]), Some(t(leap + 1001.0)));
        assert_eq!(set("milliseconds", 8.64e15, &[1.0]), None, "past the range");
        assert_eq!(set("hours", leap, &[1e20]), None);
        assert_eq!(set("date", leap, &[1.0, 2.0]), None, "setUTCDate takes one");
        assert_eq!(set("hours", leap, &[]), None);
        assert_eq!(set("day", leap, &[1.0]), None, "the weekday isn't set");
        assert_eq!(set("hours", 8.64e15 + 1.0, &[1.0]), None);
        // A text is the date it reads as, as new Date(text) makes it.
        let at = |part: &str, s: &str, x: f64| leaf("date_set", &[("part", part)], &[txt(s), t(x)]);
        assert_eq!(at("date", "2000-02-29", 0.0), Some(t(949_276_800_000.0)), "Jan 31");
        assert_eq!(at("date", "29 Feb 2000", 0.0), None);
        assert_eq!(leaf("date_text", &[("format", "iso")], &[txt("2000-02-29T12:00+01:00")]), Some(txt("2000-02-29T11:00:00.000Z")));
    }

    /// TARGET2's closing days in 2026: New Year's Day, Good Friday, Easter Monday, Labour Day,
    /// Christmas and the day after.
    fn target2() -> Value {
        Value::List(["2026-01-01", "2026-04-03", "2026-04-06", "2026-05-01", "2026-12-25", "2026-12-26"].map(txt).to_vec())
    }

    /// An offset reads a date's parts there and writes its time there, with it; `date` is the
    /// date alone. Anything but `Z`, `±HH:mm` or whole minutes up to a day is refused.
    #[test]
    fn an_offset_reads_and_writes_the_time_there() {
        let at = parsed("2026-11-06T15:30:00Z").unwrap();
        let part = |p: &str, o: Value| leaf("date_part", &[("part", p)], &[at.clone(), o]);
        assert_eq!(part("hours", txt("+01:00")), Some(t(16.0)));
        assert_eq!(part("hours", txt("-05:30")), Some(t(10.0)));
        assert_eq!(part("minutes", txt("-05:30")), Some(t(0.0)));
        assert_eq!(part("hours", t(330.0)), Some(t(21.0)), "minutes east of UTC");
        assert_eq!(part("hours", txt("Z")), Some(t(15.0)));
        assert_eq!(part("date", txt("+09:00")), Some(t(7.0)), "past midnight in Tokyo");
        assert_eq!(part("day", txt("+09:00")), Some(t(6.0)), "a Saturday there");
        assert_eq!(part("time", txt("+09:00")), part("time", txt("Z")), "the instant is the same");
        for bad in [txt("+24:00"), txt("+01:60"), txt("01:00"), txt("+1:00"), txt("+0100"), txt("z"), t(1440.0), t(1.5), Value::Boolean(true)] {
            assert_eq!(part("hours", bad.clone()), None, "{bad:?}");
        }
        let text = |f: &str, o: Option<Value>| leaf("date_text", &[("format", f)], &[Some(at.clone()), o].into_iter().flatten().collect::<Vec<_>>());
        assert_eq!(text("iso", Some(txt("+01:00"))), Some(txt("2026-11-06T16:30:00.000+01:00")));
        assert_eq!(text("iso", Some(txt("-05:30"))), Some(txt("2026-11-06T10:00:00.000-05:30")));
        assert_eq!(text("iso", Some(t(0.0))), Some(txt("2026-11-06T15:30:00.000Z")), "a zero offset is Z");
        assert_eq!(text("date", None), Some(txt("2026-11-06")));
        assert_eq!(text("date", Some(txt("+09:00"))), Some(txt("2026-11-07")));
        assert_eq!(text("utc", Some(txt("Z"))), Some(txt("Fri, 06 Nov 2026 15:30:00 GMT")));
        assert_eq!(text("utc", Some(txt("+01:00"))), None, "GMT's, not another offset's");
        assert_eq!(leaf("date_text", &[("format", "date")], &[t(-62_198_755_200_000.0)]), Some(txt("-000001-01-01")));
        for o in ["+05:45", "-11:30", "+14:00", "Z"] {
            let written = text("iso", Some(txt(o))).unwrap();
            assert_eq!(leaf("date_parse", &[], &[written]), Some(at.clone()), "read back at {o}");
        }
    }

    /// Days are 86,400,000 ms; a month keeps its day, or the month's last when it has fewer, as
    /// date-fns's addMonths, at the offset's calendar; a year is twelve months.
    #[test]
    fn date_add_clamps_a_month_to_its_last_day() {
        let add = |x: &str, n: f64, unit: &str, o: Option<&str>| {
            let mut inputs = vec![txt(x), t(n)];
            inputs.extend(o.map(txt));
            leaf("date_add", &[("unit", unit)], &inputs).and_then(|v| leaf("date_text", &[("format", "iso")], &[v]))
        };
        let day = |x: &str, n: f64, unit: &str| add(x, n, unit, None).map(|v| match v {
            Value::Text(s) => txt(&s[..10]),
            other => other,
        });
        assert_eq!(add("2026-11-06T15:30:00Z", 3.0, "days", None), Some(txt("2026-11-09T15:30:00.000Z")));
        assert_eq!(add("2026-11-06T15:30:00Z", -1.9, "days", None), Some(txt("2026-11-05T15:30:00.000Z")), "truncated");
        assert_eq!(day("2026-01-31", 1.0, "months"), Some(txt("2026-02-28")));
        assert_eq!(day("2028-01-31", 1.0, "months"), Some(txt("2028-02-29")), "a leap year");
        assert_eq!(day("2026-03-31", -1.0, "months"), Some(txt("2026-02-28")));
        assert_eq!(day("2026-01-15", 13.0, "months"), Some(txt("2027-02-15")));
        assert_eq!(day("2024-02-29", 1.0, "years"), Some(txt("2025-02-28")));
        assert_eq!(day("2024-02-29", 4.0, "years"), Some(txt("2028-02-29")));
        assert_eq!(day("2026-01-31", 12.0, "months"), day("2026-01-31", 1.0, "years"));
        // 23:30 UTC on February 28 is March 1 at +01:00: a month there is April 1, 00:30.
        assert_eq!(add("2026-02-28T23:30:00Z", 1.0, "months", Some("+01:00")), Some(txt("2026-03-31T23:30:00.000Z")));
        assert_eq!(add("2026-02-28T23:30:00Z", 1.0, "months", None), Some(txt("2026-03-28T23:30:00.000Z")));
        assert_eq!(add("2026-02-28T23:30:00Z", 1.0, "days", Some("+01:00")), add("2026-02-28T23:30:00Z", 1.0, "days", None), "a day is a day at a fixed offset");
        assert_eq!(add("2026-01-31", 1.0, "weeks", None), None);
        assert_eq!(add("2026-01-31", 1.0, "months", Some("CET")), None);
        assert_eq!(add("2026-01-31", 1e8, "months", None), None);
        assert_eq!(add("2026-01-31", 3e8, "days", None), None);
        assert_eq!(add("+275760-09-13", 1.0, "days", None), None, "past the range");
        assert_eq!(leaf("date_add", &[("unit", "days")], &[txt("2026-01-31"), txt("1")]), None, "the amount is a number");
    }

    /// Business days over TARGET2's 2026 calendar: Easter's four days closed, a weekend rolled
    /// past, other weekends, and calendar dates as texts or time values, nothing else.
    #[test]
    fn business_days_skip_the_weekend_and_the_holidays() {
        let add = |d: Value, n: f64, h: Option<Value>, weekend: &str| {
            let mut inputs = vec![d, t(n)];
            inputs.extend(h);
            leaf("business_day_add", &[("weekend", weekend)], &inputs)
        };
        let target = |d: &str, n: f64| add(txt(d), n, Some(target2()), "sat_sun");
        assert_eq!(target("2026-04-02", 1.0), Some(txt("2026-04-07")), "Good Friday, the weekend and Easter Monday");
        assert_eq!(target("2026-04-07", -1.0), Some(txt("2026-04-02")));
        assert_eq!(target("2026-02-28", 0.0), Some(txt("2026-03-02")), "a Saturday rolls to Monday");
        assert_eq!(target("2026-03-02", 0.0), Some(txt("2026-03-02")));
        assert_eq!(target("2026-02-28", 1.0), Some(txt("2026-03-02")));
        assert_eq!(target("2026-02-28", -1.0), Some(txt("2026-02-27")));
        assert_eq!(target("2026-12-24", 1.0), Some(txt("2026-12-28")));
        assert_eq!(target("2026-12-31", 1.0), Some(txt("2027-01-01")), "2027's closing days aren't in the list");
        assert_eq!(add(txt("2026-11-05"), 1.0, None, "fri_sat"), Some(txt("2026-11-08")));
        assert_eq!(add(txt("2026-11-07"), 1.0, None, "none"), Some(txt("2026-11-08")));
        assert_eq!(add(txt("2026-11-06"), 5.0, None, "sat_sun"), Some(txt("2026-11-13")));
        let late_friday = parsed("2026-11-06T23:30:00Z").unwrap();
        assert_eq!(add(late_friday, 1.0, None, "sat_sun"), Some(txt("2026-11-09")), "a time value's date in UTC");
        assert_eq!(add(txt("2026-11-06T10:00:00Z"), 1.0, None, "sat_sun"), None, "a time of day: which date is meant?");
        assert_eq!(add(txt("2026-11"), 1.0, None, "sat_sun"), None);
        assert_eq!(add(txt("2026-11-06"), 1.0, Some(Value::List(vec![txt("2026-13-01")])), "sat_sun"), None, "a holiday that isn't a date");
        assert_eq!(add(txt("2026-11-06"), 1.0, Some(txt("2026-11-09")), "sat_sun"), None, "holidays are a list");
        assert_eq!(add(txt("2026-11-06"), 1.0, Some(Value::List(vec![parsed("2026-11-09").unwrap()])), "sat_sun"), Some(txt("2026-11-10")), "a holiday as a time value");
        assert_eq!(add(txt("2026-11-06"), 1.0, None, "sun"), None);
        assert!(add(txt("2026-11-06"), 100_000.0, None, "sat_sun").is_some());
        assert_eq!(add(txt("2026-11-06"), 100_001.0, None, "sat_sun"), None, "past 100,000");
        assert_eq!(add(txt("+275760-09-12"), 1.0, None, "none"), Some(txt("+275760-09-13")));
        assert_eq!(add(txt("+275760-09-12"), 2.0, None, "none"), None, "past the range");

        let open = |d: &str| leaf("is_business_day", &[("weekend", "sat_sun")], &[txt(d), target2()]);
        assert_eq!(open("2026-12-24"), Some(Value::Boolean(true)));
        assert_eq!(open("2026-12-25"), Some(Value::Boolean(false)));
        assert_eq!(open("2026-11-08"), Some(Value::Boolean(false)), "a Sunday");
        assert_eq!(leaf("is_business_day", &[("weekend", "fri_sat")], &[txt("2026-11-08")]), Some(Value::Boolean(true)));
    }

    /// The count from `from`, not itself, to `to`, itself included, either way: the inverse of
    /// business_day_add, and what counting day by day gives, over any range.
    #[test]
    fn business_days_between_counts_what_business_day_add_steps() {
        let between = |a: &str, b: &str| leaf("business_days_between", &[("weekend", "sat_sun")], &[txt(a), txt(b), target2()]);
        assert_eq!(between("2026-01-31", "2026-03-02"), Some(t(21.0)));
        assert_eq!(between("2026-03-02", "2026-01-31"), Some(t(-20.0)), "from excluded, to included");
        assert_eq!(between("2026-04-02", "2026-04-07"), Some(t(1.0)));
        assert_eq!(between("2026-04-02", "2026-04-02"), Some(t(0.0)));
        assert_eq!(between("2026-01-01", "2026-12-31"), Some(t(256.0)), "the 260 weekdays after January 1, less four closing days: December 26 is a Saturday");
        let day = |d: i64| calendar_text(d).unwrap();
        let start = calendar_day(&txt("2026-03-20")).unwrap();
        for s in start..start + 60 {
            for n in (-12..=12).filter(|n| *n != 0) {
                let e = leaf("business_day_add", &[("weekend", "sat_sun")], &[txt(&day(s)), t(n as f64), target2()]).unwrap();
                let Value::Text(e) = e else { panic!() };
                assert_eq!(between(&day(s), &e), Some(t(n as f64)), "{} and {n} business days", day(s));
            }
        }
        let calendar = Calendar::new(Some(&target2()), "sat_sun").unwrap();
        for (a, b) in [(-3, 400), (start, start + 1), (start + 5, start - 17), (start, start + 1000), (20_000, 20_831)] {
            let stepped = if b >= a { ((a + 1)..=b).filter(|d| calendar.open(*d)).count() as f64 } else { -((b..a).filter(|d| calendar.open(*d)).count() as f64) };
            assert_eq!(between(&day(a), &day(b)), Some(t(stepped)), "{} to {}", day(a), day(b));
        }
        assert_eq!(leaf("business_days_between", &[("weekend", "sat_sun")], &[txt("-271821-04-20"), txt("+275760-09-13")]).map(|v| matches!(v, Value::Number(_))), Some(true), "the whole range, at once");
    }
}
