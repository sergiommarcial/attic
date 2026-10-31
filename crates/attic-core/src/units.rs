//! Sizes in decimal units, as Finder shows them.

const UNITS: [(&str, u64); 4] = [
    ("TB", 1_000_000_000_000),
    ("GB", 1_000_000_000),
    ("MB", 1_000_000),
    ("kB", 1_000),
];

/// Parses `"500MB"`, `"5 GB"`, `"1.5TB"` or a plain byte count. Units are
/// decimal and case-insensitive. `None` for anything else.
pub fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: f64 = number.parse().ok()?;
    let unit = unit.trim();
    let scale = if unit.is_empty() || unit.eq_ignore_ascii_case("b") {
        1
    } else {
        UNITS
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(unit))?
            .1
    };
    let bytes = number * scale as f64;
    (bytes.is_finite() && bytes <= u64::MAX as f64).then_some(bytes.round() as u64)
}

/// `31 GB`, `1.2 GB`, `950 MB`: one decimal below 10 GB-or-larger units,
/// whole numbers otherwise.
pub fn human(bytes: u64) -> String {
    let Some(mut i) = UNITS.iter().position(|(_, unit)| bytes >= *unit) else {
        return format!("{bytes} B");
    };
    // 999.6 MB would print as "1000 MB": move up a unit when rounding does.
    if i > 0 && (bytes as f64 / UNITS[i].1 as f64).round() >= 1000.0 {
        i -= 1;
    }
    let (name, unit) = UNITS[i];
    let value = bytes as f64 / unit as f64;
    if unit >= 1_000_000_000 && value < 10.0 {
        format!("{value:.1} {name}")
    } else {
        format!("{value:.0} {name}")
    }
}

/// How long ago, roughly: `today`, `3 days`, `2 weeks`, `8 months`, `2 years`.
pub fn age(days: u64) -> String {
    let (n, unit) = match days {
        0 => return "today".into(),
        1..14 => (days, "day"),
        14..60 => (days / 7, "week"),
        60..730 => (days / 30, "month"),
        _ => (days / 365, "year"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_decimal_sizes() {
        assert_eq!(parse_size("500MB"), Some(500_000_000));
        assert_eq!(parse_size("5 GB"), Some(5_000_000_000));
        assert_eq!(parse_size("750kB"), Some(750_000));
        assert_eq!(parse_size("1024"), Some(1024));
        assert_eq!(parse_size("2TB"), Some(2_000_000_000_000));
    }

    #[test]
    fn rejects_nonsense_sizes() {
        assert_eq!(parse_size("lots"), None);
        assert_eq!(parse_size(""), None);
        assert_eq!(parse_size("-5GB"), None);
        assert_eq!(parse_size("5 XB"), None);
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(999), "999 B");
        assert_eq!(human(950_000_000), "950 MB");
        assert_eq!(human(1_240_000_000), "1.2 GB");
        assert_eq!(human(31_400_000_000), "31 GB");
        assert_eq!(human(512_000_000_000), "512 GB");
        assert_eq!(human(1_000_000_000_000), "1.0 TB");
        assert_eq!(human(999_600_000), "1.0 GB");
        assert_eq!(human(999_499), "999 kB");
        assert_eq!(human(999_999), "1 MB");
    }
}

#[cfg(test)]
mod age_tests {
    use super::age;

    #[test]
    fn ages_read_naturally() {
        assert_eq!(age(0), "today");
        assert_eq!(age(1), "1 day");
        assert_eq!(age(3), "3 days");
        assert_eq!(age(14), "2 weeks");
        assert_eq!(age(240), "8 months");
        assert_eq!(age(800), "2 years");
    }
}
