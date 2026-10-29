//! 导出 / 导入用的本地时间格式化与解析。
//!
//! 不引入时区数据库：与 `dotlog-domain::day` 一致，使用记录自带的偏移量。

const MS_PER_DAY: i64 = 86_400_000;
const MS_PER_MINUTE: i64 = 60_000;

/// 公历日期 → 1970-01-01 起的天数（Howard Hinnant 算法）。
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = i64::from(month);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 1970-01-01 起的天数 → 公历日期。
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32)
}

/// `+08:00` / `-05:30` 形式的偏移文本。
pub fn offset_text(tz_offset_min: i32) -> String {
    let sign = if tz_offset_min < 0 { '-' } else { '+' };
    let total = tz_offset_min.abs();
    format!("{sign}{:02}:{:02}", total / 60, total % 60)
}

/// 本地 ISO 8601：`2026-10-28T09:00:00+08:00`。
pub fn iso_local(utc_ms: i64, tz_offset_min: i32) -> String {
    let local_ms = utc_ms + i64::from(tz_offset_min) * MS_PER_MINUTE;
    let days = local_ms.div_euclid(MS_PER_DAY);
    let today_ms = local_ms.rem_euclid(MS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let seconds = today_ms / 1_000;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}{}",
        seconds / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60,
        offset_text(tz_offset_min)
    )
}

/// 解析 ISO 8601 或 `YYYY-MM-DD HH:MM[:SS]`；无偏移时按 `fallback_tz` 解释。
pub fn parse_iso_local(text: &str, fallback_tz: i32) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let (date_part, rest) = text.split_once(['T', ' ']).unwrap_or((text, ""));
    let (year, month, day) = parse_date(date_part)?;
    if rest.is_empty() {
        return None;
    }

    // 末尾偏移：`Z` 或 `±HH:MM`。
    let (time_part, tz) = if let Some(stripped) = rest.strip_suffix('Z') {
        (stripped, 0)
    } else if let Some(offset_start) = find_offset_start(rest) {
        let (time, offset) = rest.split_at(offset_start);
        (time, parse_offset(offset)?)
    } else {
        (rest, fallback_tz)
    };

    let mut parts = time_part.split(':');
    let hour: i64 = parts.next()?.parse().ok()?;
    let minute: i64 = parts.next()?.parse().ok()?;
    let second: i64 = match parts.next() {
        Some(value) => value.split('.').next()?.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    let utc_ms = days_from_civil(year, month, day) * MS_PER_DAY
        + hour * 3_600_000
        + minute * MS_PER_MINUTE
        + second * 1_000
        - i64::from(tz) * MS_PER_MINUTE;
    Some(utc_ms)
}

/// `YYYY-MM-DD` + `HH:MM[:SS]` 组合成本地时刻（`end` 早于 `start` 视为跨零点）。
pub fn parse_date_time(date: &str, time: &str, tz_offset_min: i32) -> Option<i64> {
    let (year, month, day) = parse_date(date.trim())?;
    let mut parts = time.trim().split(':');
    let hour: i64 = parts.next()?.parse().ok()?;
    let minute: i64 = parts.next()?.parse().ok()?;
    let second: i64 = match parts.next() {
        Some(value) => value.split('.').next()?.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() || hour > 24 || minute > 59 || second > 59 {
        return None;
    }
    Some(
        days_from_civil(year, month, day) * MS_PER_DAY
            + hour * 3_600_000
            + minute * MS_PER_MINUTE
            + second * 1_000
            - i64::from(tz_offset_min) * MS_PER_MINUTE,
    )
}

fn parse_date(text: &str) -> Option<(i64, u32, u32)> {
    let mut parts = text.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if civil_from_days(days_from_civil(year, month, day)) != (year, month, day) {
        return None;
    }
    Some((year, month, day))
}

fn find_offset_start(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    (1..bytes.len())
        .rev()
        .find(|&index| bytes[index] == b'+' || bytes[index] == b'-')
}

fn parse_offset(text: &str) -> Option<i32> {
    let sign = match text.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let body = text.get(1..)?;
    let (hours, minutes) = body.split_once(':')?;
    let hours: i32 = hours.parse().ok()?;
    let minutes: i32 = minutes.parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 60 + minutes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_round_trip() {
        let ms = 1_793_000_000_000; // 2026-10-26T15:33:20+08:00
        let text = iso_local(ms, 480);
        assert_eq!(text, "2026-10-26T15:33:20+08:00");
        assert_eq!(parse_iso_local(&text, 480), Some(ms));
    }

    #[test]
    fn parses_plain_local_time() {
        let ms = parse_iso_local("2026-10-28 09:00", 480).unwrap();
        assert_eq!(iso_local(ms, 480), "2026-10-28T09:00:00+08:00");
        assert!(parse_iso_local("2026-02-31 09:00", 480).is_none());
    }

    #[test]
    fn date_time_combination() {
        let ms = parse_date_time("2026-10-28", "09:00", 480).unwrap();
        assert_eq!(ms, 1_793_149_200_000);
        assert!(parse_date_time("2026-10-28", "25:00", 480).is_none());
    }
}
