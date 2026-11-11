pub mod codec;
pub mod export;
pub mod import;
pub mod models;
pub mod proto_schema;
pub mod protobuf;
pub mod sequence;
pub mod session;
pub mod store;

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
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(byte) = decode_hex_pair(bytes[i + 1], bytes[i + 2])
        {
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

fn decode_hex_pair(high: u8, low: u8) -> Option<u8> {
    fn digit(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    Some(digit(high)? * 16 + digit(low)?)
}

pub(crate) fn hex_decode(input: &str) -> Option<Vec<u8>> {
    if !input.len().is_multiple_of(2) {
        return None;
    }
    input
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| decode_hex_pair(pair[0], pair[1]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_unicode_escapes_are_preserved() {
        assert_eq!(url_decode("%aé"), "%aé");
        assert_eq!(hex_decode("aéx"), None);
        assert_eq!(hex_decode("ff00"), Some(vec![255, 0]));
    }

    proptest::proptest! {
        #[test]
        fn decoders_never_panic(text in ".*", bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..4096)) {
            let _ = url_decode(&text);
            let _ = hex_decode(&text);
            let _ = protobuf::decode_raw(&bytes);
            let _ = protobuf::decode_grpc_body(&bytes);
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
        assert_eq!(
            extract_path("https://example.com/search?q=test"),
            "/search?q=test"
        );
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
