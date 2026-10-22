//! Consumer-side checks for SARIF path references, independent of the reporter encoder.

/// Decode a URI path once, rejecting raw non-ASCII, URI delimiters and malformed escapes.
pub fn decode_path(uri: &str) -> String {
    assert!(uri.is_ascii(), "URI must encode UTF-8 bytes: {uri:?}");
    assert!(!uri.contains(['#', '?', ' ', '[', ']', ':', '\\']));
    let mut bytes = uri.bytes();
    let mut decoded = Vec::new();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let digits = [
                bytes.next().expect("percent high digit"),
                bytes.next().expect("percent low digit"),
            ];
            let hex = std::str::from_utf8(&digits).unwrap();
            decoded.push(u8::from_str_radix(hex, 16).expect("valid percent escape"));
        } else {
            decoded.push(byte);
        }
    }
    String::from_utf8(decoded).expect("decoded path must be UTF-8")
}
