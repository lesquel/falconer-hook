//! Timestamps without a date library.

/// Unix seconds of an RFC 3339 timestamp (`2026-11-08T12:00:00.123Z`,
/// `…+02:00`). Fractional seconds are dropped; dates before 1970 are `None`.
pub fn parse_rfc3339(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = s.get(r)?;
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())?
    };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        rest = &frac[digits..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let r = rest.as_bytes();
            if r.len() != 6 || r[3] != b':' || !matches!(r[0], b'+' | b'-') {
                return None;
            }
            let oh: i64 = rest[1..3].parse().ok()?;
            let om: i64 = rest[4..6].parse().ok()?;
            let o = oh * 3600 + om * 60;
            if r[0] == b'-' {
                -o
            } else {
                o
            }
        }
    };
    let t = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset;
    u64::try_from(t).ok()
}

/// Seconds east of UTC from `date +%z` output (`+0530`, `-0500`).
pub fn parse_utc_offset(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() != 5 || !matches!(b[0], b'+' | b'-') || !s[1..].bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let h: i64 = s[1..3].parse().ok()?;
    let m: i64 = s[3..5].parse().ok()?;
    if h > 14 || m > 59 {
        return None;
    }
    let o = h * 3600 + m * 60;
    Some(if b[0] == b'-' { -o } else { o })
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
