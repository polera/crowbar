#[derive(Debug, Clone)]
pub struct ProtoField {
    pub number: u32,
    pub value: ProtoValue,
}

#[derive(Debug, Clone)]
pub enum ProtoValue {
    Varint(u64),
    Fixed64(u64),
    Fixed32(u32),
    Bytes(Vec<u8>),
    String(String),
    Message(Vec<ProtoField>),
}

impl std::fmt::Display for ProtoValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtoValue::Varint(v) => write!(f, "{}", v),
            ProtoValue::Fixed64(v) => write!(f, "{}", v),
            ProtoValue::Fixed32(v) => write!(f, "{}", v),
            ProtoValue::String(s) => {
                let truncated: std::string::String = s.chars().take(40).collect();
                if s.len() > 40 {
                    write!(f, "\"{}...\"", truncated)
                } else {
                    write!(f, "\"{}\"", truncated)
                }
            }
            ProtoValue::Bytes(b) => write!(f, "[{} bytes]", b.len()),
            ProtoValue::Message(fields) => write!(f, "{{msg: {} fields}}", fields.len()),
        }
    }
}

pub struct GrpcMessage {
    pub compressed: bool,
    pub size: usize,
    pub fields: Option<Vec<ProtoField>>,
}

pub fn decode_grpc_body(data: &[u8]) -> Vec<GrpcMessage> {
    let mut messages = Vec::new();
    let mut pos = 0;

    while pos + 5 <= data.len() {
        let compressed = data[pos];
        let len = u32::from_be_bytes([data[pos + 1], data[pos + 2], data[pos + 3], data[pos + 4]])
            as usize;

        if pos + 5 + len > data.len() {
            break;
        }

        let payload = &data[pos + 5..pos + 5 + len];
        let fields = decode_raw(payload);

        messages.push(GrpcMessage {
            compressed: compressed != 0,
            size: len,
            fields,
        });

        pos += 5 + len;
    }

    messages
}

pub fn decode_raw(data: &[u8]) -> Option<Vec<ProtoField>> {
    if data.is_empty() {
        return None;
    }

    let mut fields = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        let (tag, bytes_read) = read_varint(&data[pos..])?;
        pos += bytes_read;

        let wire_type = (tag & 0x07) as u8;
        let field_number = (tag >> 3) as u32;

        if field_number == 0 || field_number > 536_870_911 {
            return None;
        }

        match wire_type {
            0 => {
                let (value, bytes_read) = read_varint(&data[pos..])?;
                pos += bytes_read;
                fields.push(ProtoField {
                    number: field_number,
                    value: ProtoValue::Varint(value),
                });
            }
            1 => {
                if pos + 8 > data.len() {
                    return None;
                }
                let value = u64::from_le_bytes(data[pos..pos + 8].try_into().ok()?);
                pos += 8;
                fields.push(ProtoField {
                    number: field_number,
                    value: ProtoValue::Fixed64(value),
                });
            }
            2 => {
                let (len, bytes_read) = read_varint(&data[pos..])?;
                pos += bytes_read;
                let len = len as usize;
                if pos + len > data.len() {
                    return None;
                }
                let payload = &data[pos..pos + len];
                pos += len;
                fields.push(ProtoField {
                    number: field_number,
                    value: interpret_length_delimited(payload),
                });
            }
            5 => {
                if pos + 4 > data.len() {
                    return None;
                }
                let value = u32::from_le_bytes(data[pos..pos + 4].try_into().ok()?);
                pos += 4;
                fields.push(ProtoField {
                    number: field_number,
                    value: ProtoValue::Fixed32(value),
                });
            }
            _ => return None,
        }
    }

    if fields.is_empty() {
        return None;
    }

    Some(fields)
}

fn read_varint(data: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0;

    for (i, &byte) in data.iter().enumerate() {
        if shift >= 64 {
            return None;
        }
        result |= ((byte & 0x7F) as u64) << shift;
        shift += 7;

        if byte & 0x80 == 0 {
            return Some((result, i + 1));
        }
    }

    None
}

fn interpret_length_delimited(data: &[u8]) -> ProtoValue {
    if let Ok(s) = std::str::from_utf8(data)
        && !s.is_empty() && is_likely_text(s) {
            return ProtoValue::String(s.to_string());
        }

    if data.len() >= 2
        && let Some(fields) = decode_raw(data)
            && fields.iter().all(|f| f.number < 1000) {
                return ProtoValue::Message(fields);
            }

    ProtoValue::Bytes(data.to_vec())
}

fn is_likely_text(s: &str) -> bool {
    let total = s.chars().count();
    if total == 0 {
        return false;
    }
    let printable = s
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\r' || *c == '\t')
        .count();
    printable * 4 >= total * 3
}

pub fn format_fixed64(value: u64) -> String {
    let f = f64::from_bits(value);
    if f.is_finite() && f != 0.0 && f.abs() > 1e-100 && f.abs() < 1e100 {
        format!("{}", f)
    } else {
        value.to_string()
    }
}

pub fn format_fixed32(value: u32) -> String {
    let f = f32::from_bits(value);
    if f.is_finite() && f != 0.0 && f.abs() > 1e-30 && f.abs() < 1e30 {
        format!("{}", f)
    } else {
        value.to_string()
    }
}

pub fn format_proto_text(fields: &[ProtoField], indent: usize) -> Vec<String> {
    let prefix = "  ".repeat(indent);
    let mut lines = Vec::new();
    for field in fields {
        match &field.value {
            ProtoValue::Varint(v) => {
                lines.push(format!("{}{} int: {}", prefix, field.number, v));
            }
            ProtoValue::Fixed64(v) => {
                lines.push(format!("{}{} f64: {}", prefix, field.number, format_fixed64(*v)));
            }
            ProtoValue::Fixed32(v) => {
                lines.push(format!("{}{} f32: {}", prefix, field.number, format_fixed32(*v)));
            }
            ProtoValue::String(s) => {
                lines.push(format!("{}{} str: {}", prefix, field.number, s));
            }
            ProtoValue::Bytes(b) => {
                let hex: std::string::String =
                    b.iter().map(|byte| format!("{:02x}", byte)).collect();
                lines.push(format!("{}{} hex: {}", prefix, field.number, hex));
            }
            ProtoValue::Message(sub_fields) => {
                lines.push(format!("{}{} msg:", prefix, field.number));
                lines.extend(format_proto_text(sub_fields, indent + 1));
            }
        }
    }
    lines
}

pub fn parse_proto_text(lines: &[&str]) -> Option<Vec<ProtoField>> {
    let mut pos = 0;
    let fields = parse_proto_at_depth(lines, &mut pos, 0)?;
    if fields.is_empty() {
        return None;
    }
    Some(fields)
}

fn parse_proto_at_depth(
    lines: &[&str],
    pos: &mut usize,
    depth: usize,
) -> Option<Vec<ProtoField>> {
    let mut fields = Vec::new();

    while *pos < lines.len() {
        let line = lines[*pos];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            *pos += 1;
            continue;
        }

        let leading = line.len() - line.trim_start().len();
        if leading < depth * 2 {
            break;
        }

        let content = trimmed;
        let space = content.find(' ')?;
        let field_number: u32 = content[..space].parse().ok()?;
        let rest = &content[space + 1..];
        let colon = rest.find(':')?;
        let type_tag = &rest[..colon];
        let value_str = rest[colon + 1..].trim_start();

        match type_tag {
            "msg" => {
                *pos += 1;
                let sub = parse_proto_at_depth(lines, pos, depth + 1)?;
                fields.push(ProtoField {
                    number: field_number,
                    value: ProtoValue::Message(sub),
                });
            }
            _ => {
                let value = match type_tag {
                    "int" => ProtoValue::Varint(value_str.parse().ok()?),
                    "str" => ProtoValue::String(value_str.to_string()),
                    "hex" => {
                        if !value_str.len().is_multiple_of(2) {
                            return None;
                        }
                        let bytes: Vec<u8> = (0..value_str.len())
                            .step_by(2)
                            .map(|j| u8::from_str_radix(&value_str[j..j + 2], 16))
                            .collect::<Result<_, _>>()
                            .ok()?;
                        ProtoValue::Bytes(bytes)
                    }
                    "f64" => {
                        if value_str.contains('.') {
                            let f: f64 = value_str.parse().ok()?;
                            ProtoValue::Fixed64(f.to_bits())
                        } else {
                            ProtoValue::Fixed64(value_str.parse().ok()?)
                        }
                    }
                    "f32" => {
                        if value_str.contains('.') {
                            let f: f32 = value_str.parse().ok()?;
                            ProtoValue::Fixed32(f.to_bits())
                        } else {
                            ProtoValue::Fixed32(value_str.parse().ok()?)
                        }
                    }
                    _ => return None,
                };
                fields.push(ProtoField {
                    number: field_number,
                    value,
                });
                *pos += 1;
            }
        }
    }

    Some(fields)
}

pub fn encode_raw(fields: &[ProtoField]) -> Vec<u8> {
    let mut buf = Vec::new();
    for field in fields {
        encode_field(&mut buf, field);
    }
    buf
}

fn encode_field(buf: &mut Vec<u8>, field: &ProtoField) {
    let (wire_type, data) = match &field.value {
        ProtoValue::Varint(v) => {
            let mut vbuf = Vec::new();
            encode_varint(&mut vbuf, *v);
            (0u32, vbuf)
        }
        ProtoValue::Fixed64(v) => (1, v.to_le_bytes().to_vec()),
        ProtoValue::Fixed32(v) => (5, v.to_le_bytes().to_vec()),
        ProtoValue::String(s) => {
            let b = s.as_bytes();
            let mut ld = Vec::new();
            encode_varint(&mut ld, b.len() as u64);
            ld.extend_from_slice(b);
            (2, ld)
        }
        ProtoValue::Bytes(b) => {
            let mut ld = Vec::new();
            encode_varint(&mut ld, b.len() as u64);
            ld.extend_from_slice(b);
            (2, ld)
        }
        ProtoValue::Message(sub_fields) => {
            let inner = encode_raw(sub_fields);
            let mut ld = Vec::new();
            encode_varint(&mut ld, inner.len() as u64);
            ld.extend(inner);
            (2, ld)
        }
    };

    let tag = (field.number << 3) | wire_type;
    encode_varint(buf, tag as u64);
    buf.extend(data);
}

fn encode_varint(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            buf.push(byte);
            break;
        } else {
            buf.push(byte | 0x80);
        }
    }
}

pub fn encode_grpc_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(0);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_simple_varint() {
        // field 1, varint, value 150
        let data = [0x08, 0x96, 0x01];
        let fields = decode_raw(&data).unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].number, 1);
        assert!(matches!(fields[0].value, ProtoValue::Varint(150)));
    }

    #[test]
    fn decode_string_field() {
        // field 2, length-delimited, "testing"
        let data = [0x12, 0x07, 0x74, 0x65, 0x73, 0x74, 0x69, 0x6e, 0x67];
        let fields = decode_raw(&data).unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].number, 2);
        assert!(matches!(&fields[0].value, ProtoValue::String(s) if s == "testing"));
    }

    #[test]
    fn decode_multiple_fields() {
        // field 1 = 150, field 2 = "testing"
        let data = [
            0x08, 0x96, 0x01, 0x12, 0x07, 0x74, 0x65, 0x73, 0x74, 0x69, 0x6e, 0x67,
        ];
        let fields = decode_raw(&data).unwrap();
        assert_eq!(fields.len(), 2);
        assert!(matches!(fields[0].value, ProtoValue::Varint(150)));
        assert!(matches!(&fields[1].value, ProtoValue::String(s) if s == "testing"));
    }

    #[test]
    fn decode_grpc_frame() {
        // gRPC frame: not compressed, 3 bytes, field 1 = 150
        let data = [0x00, 0x00, 0x00, 0x00, 0x03, 0x08, 0x96, 0x01];
        let messages = decode_grpc_body(&data);
        assert_eq!(messages.len(), 1);
        assert!(!messages[0].compressed);
        assert_eq!(messages[0].size, 3);
        let fields = messages[0].fields.as_ref().unwrap();
        assert_eq!(fields.len(), 1);
        assert!(matches!(fields[0].value, ProtoValue::Varint(150)));
    }

    #[test]
    fn empty_data_returns_none() {
        assert!(decode_raw(&[]).is_none());
    }

    #[test]
    fn invalid_data_returns_none() {
        // Wire type 6 is invalid
        let data = [0x0E, 0x01];
        assert!(decode_raw(&data).is_none());
    }

    #[test]
    fn fixed32_field() {
        // field 5, wire type 5 (fixed32), value 0x12345678
        let data = [0x2D, 0x78, 0x56, 0x34, 0x12];
        let fields = decode_raw(&data).unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].number, 5);
        assert!(matches!(fields[0].value, ProtoValue::Fixed32(0x12345678)));
    }

    #[test]
    fn fixed64_field() {
        // field 3, wire type 1 (fixed64), 8 bytes of pi as double
        let pi_bits = f64::to_bits(std::f64::consts::PI);
        let mut data = vec![0x19]; // field 3, wire type 1
        data.extend_from_slice(&pi_bits.to_le_bytes());
        let fields = decode_raw(&data).unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].number, 3);
        if let ProtoValue::Fixed64(v) = fields[0].value {
            assert_eq!(f64::from_bits(v), std::f64::consts::PI);
        } else {
            panic!("Expected Fixed64");
        }
    }

    #[test]
    fn encode_decode_roundtrip() {
        let original = vec![
            ProtoField { number: 1, value: ProtoValue::Varint(150) },
            ProtoField { number: 2, value: ProtoValue::String("testing".into()) },
        ];
        let encoded = encode_raw(&original);
        let decoded = decode_raw(&encoded).unwrap();
        assert_eq!(decoded.len(), 2);
        assert!(matches!(decoded[0].value, ProtoValue::Varint(150)));
        assert!(matches!(&decoded[1].value, ProtoValue::String(s) if s == "testing"));
    }

    #[test]
    fn text_format_roundtrip() {
        let fields = vec![
            ProtoField { number: 1, value: ProtoValue::Varint(42) },
            ProtoField { number: 2, value: ProtoValue::String("hello".into()) },
            ProtoField { number: 3, value: ProtoValue::Bytes(vec![0xde, 0xad]) },
            ProtoField {
                number: 4,
                value: ProtoValue::Message(vec![
                    ProtoField { number: 1, value: ProtoValue::Varint(99) },
                ]),
            },
        ];
        let text = format_proto_text(&fields, 0);
        let refs: Vec<&str> = text.iter().map(|s| s.as_str()).collect();
        let parsed = parse_proto_text(&refs).unwrap();
        let re_encoded = encode_raw(&parsed);
        let original_encoded = encode_raw(&fields);
        assert_eq!(re_encoded, original_encoded);
    }

    #[test]
    fn grpc_frame_encode_roundtrip() {
        let payload = encode_raw(&[
            ProtoField { number: 1, value: ProtoValue::Varint(150) },
        ]);
        let frame = encode_grpc_frame(&payload);
        let messages = decode_grpc_body(&frame);
        assert_eq!(messages.len(), 1);
        assert!(!messages[0].compressed);
        let decoded = messages[0].fields.as_ref().unwrap();
        assert!(matches!(decoded[0].value, ProtoValue::Varint(150)));
    }
}
