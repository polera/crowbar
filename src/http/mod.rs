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
