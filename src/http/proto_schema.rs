//! Schema-aware gRPC/protobuf decoding.
//!
//! When the user points crowbar at one or more directories of `.proto` files
//! (via `--proto-dir` / config), this module compiles them into a descriptor
//! pool and resolves each gRPC call's `/package.Service/Method` path to the
//! method's request/response message type. Payloads are then decoded with
//! field *names* and accurate types (enums by name, zigzag `sint`, etc.).
//!
//! The output is the same "named text format" used by the editor, an extension
//! of the heuristic format in [`crate::http::protobuf`]:
//!
//! ```text
//! 1 user_id int: 42
//! 2 name str: alice
//! 3 role enum: ROLE_ADMIN
//! 4 tags str: a
//! 4 tags str: b
//! 5 inner msg:
//!   1 x int: 99
//! ```
//!
//! Decoding/encoding round-trips through `prost-reflect`'s `DynamicMessage`, so
//! the wire bytes are produced by a real protobuf encoder. When no schema is
//! loaded, or a method/type can't be resolved, the caller falls back to the
//! schema-agnostic heuristic decoder and behavior is unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use prost::Message;
use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MapKey, MessageDescriptor, Value,
};

static REGISTRY: OnceLock<ProtoRegistry> = OnceLock::new();

pub struct ProtoRegistry {
    pool: DescriptorPool,
}

/// Compile every `.proto` found under `dirs` (recursively) into a descriptor
/// pool and install it as the process-wide registry. `includes` are extra
/// import roots for protos that import across directories. Returns the number
/// of message types loaded.
///
/// This is additive: a failure here should be surfaced as a warning by the
/// caller and crowbar should keep running with heuristic decoding.
pub fn init(dirs: &[PathBuf], includes: &[PathBuf]) -> anyhow::Result<usize> {
    let pool = build_pool(dirs, includes)?;
    let count = pool.all_messages().count();
    // `set` only fails if already initialized; ignore so re-init is a no-op.
    let _ = REGISTRY.set(ProtoRegistry { pool });
    Ok(count)
}

/// Compile every `.proto` under `dirs` into a descriptor pool, using `dirs`
/// plus `includes` as import roots. Pure (no global state) so it can be tested
/// directly.
fn build_pool(dirs: &[PathBuf], includes: &[PathBuf]) -> anyhow::Result<DescriptorPool> {
    let mut files = Vec::new();
    for dir in dirs {
        collect_protos(dir, &mut files)?;
    }
    if files.is_empty() {
        anyhow::bail!("no .proto files found under {:?}", dirs);
    }

    // The proto dirs double as import roots, plus any explicit include paths.
    let mut include_paths: Vec<PathBuf> = dirs.to_vec();
    include_paths.extend(includes.iter().cloned());

    let fds = protox::compile(&files, &include_paths)
        .map_err(|e| anyhow::anyhow!("failed to compile .proto files: {e}"))?;
    DescriptorPool::from_file_descriptor_set(fds)
        .map_err(|e| anyhow::anyhow!("failed to build descriptor pool: {e}"))
}

pub fn registry() -> Option<&'static ProtoRegistry> {
    REGISTRY.get()
}

fn collect_protos(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)
        .map_err(|e| anyhow::anyhow!("cannot read proto dir {}: {e}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_protos(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "proto") {
            out.push(path);
        }
    }
    Ok(())
}

impl ProtoRegistry {
    /// Resolve a gRPC `:path` (full URI or `/pkg.Service/Method`) to its method.
    fn method_for_uri(&self, uri: &str) -> Option<prost_reflect::MethodDescriptor> {
        let path = crate::http::extract_path(uri);
        let path = path.split('?').next().unwrap_or(path);
        let trimmed = path.trim_start_matches('/');
        let (service, method) = trimmed.rsplit_once('/')?;
        self.pool
            .get_service_by_name(service)?
            .methods()
            .find(|m| m.name() == method)
    }
}

/// The request (input) message type for a gRPC call, if a schema resolves it.
pub fn request_type(uri: &str) -> Option<MessageDescriptor> {
    Some(registry()?.method_for_uri(uri)?.input())
}

/// The response (output) message type for a gRPC call, if a schema resolves it.
pub fn response_type(uri: &str) -> Option<MessageDescriptor> {
    Some(registry()?.method_for_uri(uri)?.output())
}

// ---------------------------------------------------------------------------
// Decode: bytes -> named text lines
// ---------------------------------------------------------------------------

/// Decode a single protobuf message payload against `desc` into named text
/// lines. Returns `None` if the bytes don't parse against the schema (the
/// caller then falls back to heuristic decoding).
pub fn decode_message_text(
    desc: &MessageDescriptor,
    payload: &[u8],
    indent: usize,
) -> Option<Vec<String>> {
    let msg = DynamicMessage::decode(desc.clone(), payload).ok()?;
    let mut out = Vec::new();
    render_fields(&msg, indent, &mut out);
    Some(out)
}

fn render_fields(msg: &DynamicMessage, indent: usize, out: &mut Vec<String>) {
    // `fields()` yields only populated fields; sort by number for stable output.
    let mut fields: Vec<(FieldDescriptor, &Value)> = msg.fields().collect();
    fields.sort_by_key(|(fd, _)| fd.number());
    for (fd, value) in fields {
        render_field(&fd, value, indent, out);
    }
}

fn render_field(fd: &FieldDescriptor, value: &Value, indent: usize, out: &mut Vec<String>) {
    if fd.is_list() {
        if let Value::List(items) = value {
            for item in items {
                render_singular(fd, item, indent, out);
            }
        }
    } else if fd.is_map() {
        if let Value::Map(entries) = value {
            out.push(format!("{}{} {} map:", pad(indent), fd.number(), fd.name()));
            render_map_entries(fd, entries, indent + 1, out);
        }
    } else {
        render_singular(fd, value, indent, out);
    }
}

fn render_singular(fd: &FieldDescriptor, value: &Value, indent: usize, out: &mut Vec<String>) {
    let prefix = pad(indent);
    match fd.kind() {
        Kind::Message(_) => {
            out.push(format!("{}{} {} msg:", prefix, fd.number(), fd.name()));
            if let Value::Message(inner) = value {
                render_fields(inner, indent + 1, out);
            }
        }
        Kind::Enum(ed) => {
            let n = value.as_enum_number().unwrap_or(0);
            let name = ed
                .get_value(n)
                .map(|v| v.name().to_string())
                .unwrap_or_else(|| n.to_string());
            out.push(format!(
                "{}{} {} enum: {}",
                prefix,
                fd.number(),
                fd.name(),
                name
            ));
        }
        kind => {
            out.push(format!(
                "{}{} {} {}: {}",
                prefix,
                fd.number(),
                fd.name(),
                kind_tag(&kind),
                scalar_repr(value),
            ));
        }
    }
}

fn render_map_entries(
    fd: &FieldDescriptor,
    entries: &HashMap<MapKey, Value>,
    indent: usize,
    out: &mut Vec<String>,
) {
    let Kind::Message(entry_md) = fd.kind() else {
        return;
    };
    let Some(val_fd) = entry_md.get_field(2) else {
        return;
    };
    let prefix = pad(indent);
    // Sort entries by key text for deterministic output.
    let mut pairs: Vec<(String, &Value)> =
        entries.iter().map(|(k, v)| (mapkey_repr(k), v)).collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    for (key, value) in pairs {
        match val_fd.kind() {
            Kind::Message(_) => {
                out.push(format!("{}{} => msg:", prefix, key));
                if let Value::Message(inner) = value {
                    render_fields(inner, indent + 1, out);
                }
            }
            Kind::Enum(ed) => {
                let n = value.as_enum_number().unwrap_or(0);
                let name = ed
                    .get_value(n)
                    .map(|v| v.name().to_string())
                    .unwrap_or_else(|| n.to_string());
                out.push(format!("{}{} => enum: {}", prefix, key, name));
            }
            kind => {
                out.push(format!(
                    "{}{} => {}: {}",
                    prefix,
                    key,
                    kind_tag(&kind),
                    scalar_repr(value)
                ));
            }
        }
    }
}

fn kind_tag(kind: &Kind) -> &'static str {
    match kind {
        Kind::Double => "f64",
        Kind::Float => "f32",
        Kind::Int32 | Kind::Int64 => "int",
        Kind::Uint32 | Kind::Uint64 => "uint",
        Kind::Sint32 | Kind::Sint64 => "sint",
        Kind::Fixed32 | Kind::Fixed64 => "fixed",
        Kind::Sfixed32 | Kind::Sfixed64 => "sfixed",
        Kind::Bool => "bool",
        Kind::String => "str",
        Kind::Bytes => "hex",
        Kind::Enum(_) => "enum",
        Kind::Message(_) => "msg",
    }
}

fn scalar_repr(value: &Value) -> String {
    match value {
        Value::Bool(b) => b.to_string(),
        Value::I32(v) => v.to_string(),
        Value::I64(v) => v.to_string(),
        Value::U32(v) => v.to_string(),
        Value::U64(v) => v.to_string(),
        Value::F32(v) => format!("{}", v),
        Value::F64(v) => format!("{}", v),
        Value::String(s) => s.clone(),
        Value::Bytes(b) => hex_encode(b),
        Value::EnumNumber(n) => n.to_string(),
        // Message/List/Map are handled by callers; render a marker if reached.
        _ => String::new(),
    }
}

fn mapkey_repr(key: &MapKey) -> String {
    match key {
        MapKey::Bool(b) => b.to_string(),
        MapKey::I32(v) => v.to_string(),
        MapKey::I64(v) => v.to_string(),
        MapKey::U32(v) => v.to_string(),
        MapKey::U64(v) => v.to_string(),
        // Quote string keys so they survive the " => " split on parse.
        MapKey::String(s) => format!("\"{}\"", s),
    }
}

fn pad(indent: usize) -> String {
    "  ".repeat(indent)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
        .collect::<Result<_, _>>()
        .ok()
}

// ---------------------------------------------------------------------------
// Encode: named text lines -> bytes
// ---------------------------------------------------------------------------

/// Encode named text `lines` back into protobuf wire bytes against `desc`.
/// Returns `None` if the text can't be parsed (the caller then falls back to
/// the heuristic encoder).
pub fn encode_message_text(desc: &MessageDescriptor, lines: &[&str]) -> Option<Vec<u8>> {
    let mut pos = 0;
    let msg = parse_message(desc, lines, &mut pos, 0)?;
    Some(msg.encode_to_vec())
}

fn leading_depth(line: &str) -> usize {
    (line.len() - line.trim_start().len()) / 2
}

fn parse_message(
    desc: &MessageDescriptor,
    lines: &[&str],
    pos: &mut usize,
    depth: usize,
) -> Option<DynamicMessage> {
    let mut msg = DynamicMessage::new(desc.clone());
    let mut lists: HashMap<u32, Vec<Value>> = HashMap::new();

    while *pos < lines.len() {
        let line = lines[*pos];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            *pos += 1;
            continue;
        }
        if leading_depth(line) < depth {
            break;
        }

        // "<number> <name> <tag>: <value>" / "<number> <name> msg:" / "... map:"
        let mut it = trimmed.splitn(3, ' ');
        let number: u32 = it.next()?.parse().ok()?;
        let _name = it.next()?; // human aid; resolution is by number
        let remainder = it.next().unwrap_or("");

        let Some(fd) = desc.get_field(number) else {
            // Unknown field: skip the line (and any indented block under it).
            *pos += 1;
            skip_block(lines, pos, depth + 1);
            continue;
        };
        *pos += 1;

        if fd.is_map() {
            let map = parse_map(&fd, lines, pos, depth + 1)?;
            msg.set_field(&fd, Value::Map(map));
        } else if let Kind::Message(md) = fd.kind() {
            let inner = parse_message(&md, lines, pos, depth + 1)?;
            if fd.is_list() {
                lists.entry(number).or_default().push(Value::Message(inner));
            } else {
                msg.set_field(&fd, Value::Message(inner));
            }
        } else {
            let value_str = remainder.split_once(':').map(|(_, v)| v.trim_start())?;
            let value = parse_scalar(&fd, value_str)?;
            if fd.is_list() {
                lists.entry(number).or_default().push(value);
            } else {
                msg.set_field(&fd, value);
            }
        }
    }

    for (number, values) in lists {
        let fd = desc.get_field(number)?;
        msg.set_field(&fd, Value::List(values));
    }

    Some(msg)
}

/// Skip an indented child block (used when an unknown field number appears).
fn skip_block(lines: &[&str], pos: &mut usize, depth: usize) {
    while *pos < lines.len() {
        let line = lines[*pos];
        if line.trim().is_empty() {
            *pos += 1;
            continue;
        }
        if leading_depth(line) < depth {
            break;
        }
        *pos += 1;
    }
}

fn parse_map(
    fd: &FieldDescriptor,
    lines: &[&str],
    pos: &mut usize,
    depth: usize,
) -> Option<HashMap<MapKey, Value>> {
    let Kind::Message(entry_md) = fd.kind() else {
        return None;
    };
    let key_fd = entry_md.get_field(1)?;
    let val_fd = entry_md.get_field(2)?;
    let mut map = HashMap::new();

    while *pos < lines.len() {
        let line = lines[*pos];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            *pos += 1;
            continue;
        }
        if leading_depth(line) < depth {
            break;
        }

        let arrow = trimmed.find(" => ")?;
        let key_part = trimmed[..arrow].trim();
        let rest = &trimmed[arrow + 4..];
        *pos += 1;

        let key = parse_mapkey(&key_fd, key_part)?;
        let value = if let Kind::Message(md) = val_fd.kind() {
            Value::Message(parse_message(&md, lines, pos, depth + 1)?)
        } else {
            let value_str = rest.split_once(':').map(|(_, v)| v.trim_start())?;
            parse_scalar(&val_fd, value_str)?
        };
        map.insert(key, value);
    }

    Some(map)
}

fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

fn parse_scalar(fd: &FieldDescriptor, s: &str) -> Option<Value> {
    Some(match fd.kind() {
        Kind::Double => Value::F64(s.parse().ok()?),
        Kind::Float => Value::F32(s.parse().ok()?),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => Value::I32(s.parse().ok()?),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => Value::I64(s.parse().ok()?),
        Kind::Uint32 | Kind::Fixed32 => Value::U32(s.parse().ok()?),
        Kind::Uint64 | Kind::Fixed64 => Value::U64(s.parse().ok()?),
        Kind::Bool => Value::Bool(parse_bool(s)?),
        Kind::String => Value::String(s.to_string()),
        Kind::Bytes => Value::Bytes(hex_decode(s)?.into()),
        Kind::Enum(ed) => {
            let n = ed
                .get_value_by_name(s)
                .map(|v| v.number())
                .or_else(|| s.parse::<i32>().ok())?;
            Value::EnumNumber(n)
        }
        Kind::Message(_) => return None,
    })
}

fn parse_mapkey(fd: &FieldDescriptor, s: &str) -> Option<MapKey> {
    Some(match fd.kind() {
        Kind::Bool => MapKey::Bool(parse_bool(s)?),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => MapKey::I32(s.parse().ok()?),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => MapKey::I64(s.parse().ok()?),
        Kind::Uint32 | Kind::Fixed32 => MapKey::U32(s.parse().ok()?),
        Kind::Uint64 | Kind::Fixed64 => MapKey::U64(s.parse().ok()?),
        Kind::String => MapKey::String(s.trim_matches('"').to_string()),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> DescriptorPool {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proto");
        let proto = format!("{dir}/sample.proto");
        let fds = protox::compile([proto], [dir]).expect("compile sample.proto");
        DescriptorPool::from_file_descriptor_set(fds).expect("build pool")
    }

    fn user_msg() -> MessageDescriptor {
        pool()
            .get_message_by_name("sample.User")
            .expect("User type")
    }

    /// Encode `msg`, decode to text, re-encode the text, and assert the two
    /// messages are structurally equal — i.e. the named-text round-trip is
    /// lossless at the protobuf level.
    fn assert_text_roundtrip(desc: &MessageDescriptor, msg: &DynamicMessage) {
        let original = msg.encode_to_vec();
        let lines = decode_message_text(desc, &original, 0).expect("decode to text");
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let reencoded = encode_message_text(desc, &refs).expect("encode from text");
        let a = DynamicMessage::decode(desc.clone(), original.as_slice()).unwrap();
        let b = DynamicMessage::decode(desc.clone(), reencoded.as_slice()).unwrap();
        assert_eq!(a, b, "round-trip mismatch; text:\n{}", lines.join("\n"));
    }

    #[test]
    fn resolves_method_input_output() {
        let pool = pool();
        let svc = pool.get_service_by_name("sample.UserService").unwrap();
        let method = svc.methods().find(|m| m.name() == "GetUser").unwrap();
        assert_eq!(method.input().full_name(), "sample.GetUserRequest");
        assert_eq!(method.output().full_name(), "sample.User");
    }

    #[test]
    fn decodes_named_fields_and_enum() {
        let desc = user_msg();
        // Build a User via the schema, encode, then decode to text.
        let mut msg = DynamicMessage::new(desc.clone());
        for f in desc.fields() {
            match f.name() {
                "id" => msg.set_field(&f, Value::I64(42)),
                "name" => msg.set_field(&f, Value::String("alice".into())),
                "role" => msg.set_field(&f, Value::EnumNumber(1)), // ROLE_ADMIN
                "tags" => msg.set_field(
                    &f,
                    Value::List(vec![Value::String("a".into()), Value::String("b".into())]),
                ),
                _ => {}
            }
        }
        let bytes = msg.encode_to_vec();

        let lines = decode_message_text(&desc, &bytes, 0).unwrap();
        let text = lines.join("\n");
        assert!(text.contains("1 id int: 42"), "got:\n{text}");
        assert!(text.contains("2 name str: alice"), "got:\n{text}");
        assert!(text.contains("role enum: ROLE_ADMIN"), "got:\n{text}");
        assert!(text.contains("4 tags str: a"), "got:\n{text}");
        assert!(text.contains("4 tags str: b"), "got:\n{text}");
    }

    #[test]
    fn round_trip_preserves_wire_bytes() {
        let desc = user_msg();
        let mut msg = DynamicMessage::new(desc.clone());
        for f in desc.fields() {
            match f.name() {
                "id" => msg.set_field(&f, Value::I64(7)),
                "name" => msg.set_field(&f, Value::String("bob".into())),
                "role" => msg.set_field(&f, Value::EnumNumber(2)),
                "tags" => msg.set_field(
                    &f,
                    Value::List(vec![Value::String("x".into()), Value::String("y".into())]),
                ),
                _ => {}
            }
        }
        let original = msg.encode_to_vec();

        let lines = decode_message_text(&desc, &original, 0).unwrap();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let reencoded = encode_message_text(&desc, &refs).unwrap();

        // Re-decode both and compare structurally (wire field order is stable
        // here, but compare via decode to be robust).
        let a = DynamicMessage::decode(desc.clone(), original.as_slice()).unwrap();
        let b = DynamicMessage::decode(desc.clone(), reencoded.as_slice()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn nested_message_round_trip() {
        let pool = pool();
        let desc = pool.get_message_by_name("sample.GetUserRequest").unwrap();
        let lines = ["1 user_id int: 99", "2 opts msg:", "  1 verbose bool: true"];
        let bytes = encode_message_text(&desc, &lines).unwrap();
        let back = decode_message_text(&desc, &bytes, 0).unwrap().join("\n");
        assert!(back.contains("1 user_id int: 99"), "got:\n{back}");
        assert!(back.contains("verbose bool: true"), "got:\n{back}");
    }

    #[test]
    fn unresolvable_path_returns_none_without_registry() {
        // No registry installed in this test binary's default state.
        assert!(request_type("/does.Not/Exist").is_none());
        assert!(response_type("/does.Not/Exist").is_none());
    }

    #[test]
    fn map_round_trip_and_render() {
        let desc = user_msg();
        let mut msg = DynamicMessage::new(desc.clone());
        let attrs = desc.get_field_by_name("attributes").unwrap();
        let mut map = HashMap::new();
        map.insert(MapKey::String("env".into()), Value::String("prod".into()));
        map.insert(MapKey::String("tier".into()), Value::String("gold".into()));
        msg.set_field(&attrs, Value::Map(map));
        msg.set_field(&desc.get_field_by_name("id").unwrap(), Value::I64(1));

        let text = decode_message_text(&desc, &msg.encode_to_vec(), 0)
            .unwrap()
            .join("\n");
        assert!(text.contains("5 attributes map:"), "got:\n{text}");
        assert!(text.contains("\"env\" => str: prod"), "got:\n{text}");
        assert!(text.contains("\"tier\" => str: gold"), "got:\n{text}");

        assert_text_roundtrip(&desc, &msg);
    }

    #[test]
    fn all_scalar_types_round_trip() {
        let pool = pool();
        let desc = pool.get_message_by_name("sample.Scalars").unwrap();
        let mut msg = DynamicMessage::new(desc.clone());
        for f in desc.fields() {
            match f.name() {
                "sint32_val" => msg.set_field(&f, Value::I32(-7)),
                "sint64_val" => msg.set_field(&f, Value::I64(-9_000_000_000)),
                "fixed32_val" => msg.set_field(&f, Value::U32(u32::MAX)),
                "fixed64_val" => msg.set_field(&f, Value::U64(u64::MAX)),
                "sfixed32_val" => msg.set_field(&f, Value::I32(i32::MIN)),
                "sfixed64_val" => msg.set_field(&f, Value::I64(-1)),
                "float_val" => msg.set_field(&f, Value::F32(1.5)),
                "double_val" => msg.set_field(&f, Value::F64(2.25)),
                "uint32_val" => msg.set_field(&f, Value::U32(123)),
                "uint64_val" => msg.set_field(&f, Value::U64(456)),
                "bytes_val" => msg.set_field(&f, Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef].into())),
                _ => {}
            }
        }

        let text = decode_message_text(&desc, &msg.encode_to_vec(), 0)
            .unwrap()
            .join("\n");
        // Tags must reflect the schema's wire type, and signed/zigzag values
        // must survive — exactly what the heuristic decoder cannot do.
        assert!(text.contains("sint32_val sint: -7"), "got:\n{text}");
        assert!(
            text.contains("sint64_val sint: -9000000000"),
            "got:\n{text}"
        );
        assert!(
            text.contains("fixed32_val fixed: 4294967295"),
            "got:\n{text}"
        );
        assert!(
            text.contains("sfixed32_val sfixed: -2147483648"),
            "got:\n{text}"
        );
        assert!(text.contains("float_val f32: 1.5"), "got:\n{text}");
        assert!(text.contains("double_val f64: 2.25"), "got:\n{text}");
        assert!(text.contains("uint32_val uint: 123"), "got:\n{text}");
        assert!(text.contains("bytes_val hex: deadbeef"), "got:\n{text}");

        assert_text_roundtrip(&desc, &msg);
    }

    #[test]
    fn malformed_bytes_return_none() {
        let desc = user_msg();
        // Continuation bits set with no terminating byte: invalid wire format.
        assert!(decode_message_text(&desc, &[0xFF, 0xFF, 0xFF], 0).is_none());
    }

    #[test]
    fn unknown_enum_number_renders_as_number() {
        let desc = user_msg();
        let mut msg = DynamicMessage::new(desc.clone());
        msg.set_field(
            &desc.get_field_by_name("role").unwrap(),
            Value::EnumNumber(99),
        );
        let text = decode_message_text(&desc, &msg.encode_to_vec(), 0)
            .unwrap()
            .join("\n");
        assert!(text.contains("role enum: 99"), "got:\n{text}");
    }

    #[test]
    fn encode_skips_unknown_field_numbers() {
        let pool = pool();
        let desc = pool.get_message_by_name("sample.GetUserRequest").unwrap();
        // Field 1 is known; 98/99 are not (99 even carries an indented block).
        let lines = [
            "1 user_id int: 5",
            "98 bogus int: 1",
            "99 ghost msg:",
            "  1 x int: 2",
        ];
        let bytes = encode_message_text(&desc, &lines).unwrap();
        let back = decode_message_text(&desc, &bytes, 0).unwrap().join("\n");
        assert!(back.contains("1 user_id int: 5"), "got:\n{back}");
        assert!(!back.contains("bogus"), "got:\n{back}");
        assert!(!back.contains("ghost"), "got:\n{back}");
    }

    #[test]
    fn method_for_uri_parses_variants() {
        let reg = ProtoRegistry { pool: pool() };
        assert!(reg.method_for_uri("/sample.UserService/GetUser").is_some());
        // Full URL with a query string.
        assert!(
            reg.method_for_uri("https://h/sample.UserService/GetUser?x=1")
                .is_some()
        );
        // Unknown method on a known service.
        assert!(reg.method_for_uri("/sample.UserService/Nope").is_none());
        // Missing the method segment entirely.
        assert!(reg.method_for_uri("/sample.UserService").is_none());
    }

    #[test]
    fn collect_protos_recurses_into_subdirs() {
        let dir: PathBuf = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proto").into();
        let mut files = Vec::new();
        collect_protos(&dir, &mut files).unwrap();
        assert!(
            files.iter().any(|p| p.ends_with("sample.proto")),
            "files: {files:?}"
        );
        assert!(
            files.iter().any(|p| p.ends_with("nested/extra.proto")),
            "recursion missed nested dir; files: {files:?}"
        );
    }

    #[test]
    fn build_pool_resolves_cross_dir_import_with_include() {
        let importer: PathBuf =
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proto_importer").into();
        let common: PathBuf =
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/proto_common").into();

        // With the extra include root, the cross-dir import resolves.
        let pool =
            build_pool(std::slice::from_ref(&importer), &[common]).expect("compile with include");
        assert!(pool.get_message_by_name("importer.Wrapper").is_some());
        assert!(pool.get_message_by_name("common.Common").is_some());

        // Without it, the import is unresolved and compilation fails.
        assert!(build_pool(&[importer], &[]).is_err());
    }
}
