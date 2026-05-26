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
