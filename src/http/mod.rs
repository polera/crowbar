pub mod codec;
pub mod export;
pub mod import;
pub mod models;
pub mod protobuf;
pub mod sequence;
pub mod session;
pub mod store;

pub(crate) fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

pub(crate) fn month_lengths(y: u64) -> [u64; 12] {
    if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    }
}

pub(crate) fn days_to_date(days: u64) -> (u64, u64, u64) {
    let mut y = 1970;
    let mut remaining = days;

    loop {
        let year_days = if is_leap(y) { 366 } else { 365 };
        if remaining < year_days {
            break;
        }
        remaining -= year_days;
        y += 1;
    }

    let mut m = 0;
    for days_in_month in month_lengths(y) {
        if remaining < days_in_month {
            break;
        }
        remaining -= days_in_month;
        m += 1;
    }

    (y, m + 1, remaining + 1)
}

pub(crate) fn date_to_days(year: u64, month: u64, day: u64) -> u64 {
    let mut days: u64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    for m in month_lengths(year).iter().take((month as usize).saturating_sub(1)) {
        days += m;
    }
    days + day.saturating_sub(1)
}

pub(crate) fn extract_path(uri: &str) -> &str {
    if let Some(pos) = uri.find("://") {
        let after_scheme = &uri[pos + 3..];
        after_scheme
            .find('/')
            .map(|i| &after_scheme[i..])
            .unwrap_or("/")
    } else if uri.starts_with('/') {
        uri
    } else {
        "/"
    }
}

pub(crate) fn url_decode(input: &str) -> String {
    let mut result = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(
                &input[i + 1..i + 3],
                16,
            ) {
                result.push(byte);
                i += 3;
                continue;
            }
        if bytes[i] == b'+' {
            result.push(b' ');
        } else {
            result.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&result).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leap_year_divisible_by_4() {
        assert!(is_leap(2024));
        assert!(is_leap(2004));
    }

    #[test]
    fn leap_year_century_not_leap() {
        assert!(!is_leap(1900));
        assert!(!is_leap(2100));
    }

    #[test]
    fn leap_year_400_is_leap() {
        assert!(is_leap(2000));
        assert!(is_leap(1600));
    }

    #[test]
    fn non_leap_year() {
        assert!(!is_leap(2001));
        assert!(!is_leap(2023));
    }

    #[test]
    fn month_lengths_leap() {
        let months = month_lengths(2024);
        assert_eq!(months[1], 29);
        assert_eq!(months[0], 31);
        assert_eq!(months[3], 30);
    }

    #[test]
    fn month_lengths_non_leap() {
        let months = month_lengths(2023);
        assert_eq!(months[1], 28);
    }

    #[test]
    fn days_to_date_epoch() {
        assert_eq!(days_to_date(0), (1970, 1, 1));
    }

    #[test]
    fn days_to_date_known_date() {
        // 2024-01-01 is 19723 days from epoch
        let (y, m, d) = days_to_date(19723);
        assert_eq!((y, m, d), (2024, 1, 1));
    }

    #[test]
    fn days_to_date_feb_29_leap() {
        // 2024-02-29
        let days = date_to_days(2024, 2, 29);
        assert_eq!(days_to_date(days), (2024, 2, 29));
    }

    #[test]
    fn date_to_days_epoch() {
        assert_eq!(date_to_days(1970, 1, 1), 0);
    }

    #[test]
    fn date_days_roundtrip() {
        for (y, m, d) in [(1970, 1, 1), (2000, 6, 15), (2024, 12, 31), (1999, 2, 28)] {
            let days = date_to_days(y, m, d);
            assert_eq!(days_to_date(days), (y, m, d), "roundtrip failed for {y}-{m}-{d}");
        }
    }

    #[test]
    fn extract_path_with_scheme() {
        assert_eq!(extract_path("https://example.com/foo/bar"), "/foo/bar");
    }

    #[test]
    fn extract_path_with_scheme_no_path() {
        assert_eq!(extract_path("https://example.com"), "/");
    }

    #[test]
    fn extract_path_already_path() {
        assert_eq!(extract_path("/api/v1"), "/api/v1");
    }

    #[test]
    fn extract_path_no_scheme_no_slash() {
        assert_eq!(extract_path("example.com"), "/");
    }

    #[test]
    fn extract_path_with_query() {
        assert_eq!(extract_path("https://example.com/search?q=test"), "/search?q=test");
    }

    #[test]
    fn url_decode_percent_encoding() {
        assert_eq!(url_decode("hello%20world"), "hello world");
        assert_eq!(url_decode("%3Fquery%3Dvalue"), "?query=value");
    }

    #[test]
    fn url_decode_plus_as_space() {
        assert_eq!(url_decode("hello+world"), "hello world");
    }

    #[test]
    fn url_decode_no_encoding() {
        assert_eq!(url_decode("plain"), "plain");
    }

    #[test]
    fn url_decode_mixed() {
        assert_eq!(url_decode("a+b%26c"), "a b&c");
    }

    #[test]
    fn url_decode_incomplete_percent() {
        assert_eq!(url_decode("abc%2"), "abc%2");
    }

    #[test]
    fn url_decode_invalid_hex() {
        assert_eq!(url_decode("abc%ZZ"), "abc%ZZ");
    }
}
