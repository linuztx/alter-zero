//! The streamable-HTTP **request-metadata headers** (`docs/tools/mcp.md`):
//! the value encoding the `Mcp-Name` and `Mcp-Param-{Name}` headers share,
//! and the `x-mcp-header` tool-schema extension (2026-07-28, SEP-2243) — a
//! server marking tool parameters whose values must also ride the request
//! as headers, so a gateway can route on them without reading the body.
//!
//! A client on streamable HTTP **MUST** support it: mirror each annotated
//! parameter's value into `Mcp-Param-{Name}` ([`param_header_values`]) and
//! refuse a tool whose annotations break the rules ([`param_headers`]) —
//! leaving that one tool out rather than every call to it failing with
//! `400 HeaderMismatch`. The model never sees the annotation
//! ([`strip_header_annotations`]).
//!
//! The rules follow the spec and the reference TypeScript SDK's
//! `mcpParamHeaders.ts`: an RFC 9110 token for a name, case-insensitively
//! unique, only on a property typed `string`, `integer` or `boolean`, and
//! only where a chain of `properties` keys reaches it.

use serde_json::Value;

/// The prefix every mirrored parameter header carries.
pub const MCP_PARAM_HEADER_PREFIX: &str = "Mcp-Param-";

/// The schema-extension key a tool's `inputSchema` marks a parameter with.
pub const X_MCP_HEADER_KEY: &str = "x-mcp-header";

/// One `x-mcp-header` declaration: the parameter's path from the arguments
/// root (a chain of `properties` keys) and the `{Name}` its header carries,
/// case kept as declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamHeader {
    pub path: Vec<String>,
    pub name: String,
}

/// A value for the `Mcp-Name`/`Mcp-Param-{Name}` headers: verbatim when it is
/// printable ASCII with no leading/trailing whitespace and doesn't itself
/// match the `=?base64?…?=` sentinel; else that sentinel around the standard
/// base64 (padded) of its UTF-8. The empty string is encoded too, as the
/// reference SDK does — an intermediary may drop an empty header, while every
/// server decodes the sentinel. Tool names are only SHOULD-constrained to
/// header-safe characters, so the encoding path is real, not theoretical.
#[must_use]
pub fn header_value(text: &str) -> String {
    let ascii_printable = text.bytes().all(|b| (0x20..=0x7e).contains(&b));
    let whitespace_framed = text.starts_with(' ') || text.ends_with(' ');
    let looks_like_sentinel = text.starts_with("=?base64?") && text.ends_with("?=");
    if !text.is_empty() && ascii_printable && !whitespace_framed && !looks_like_sentinel {
        return text.to_string();
    }
    // Standard base64 (RFC 4648 §4, with padding) — the sentinel's alphabet.
    format!(
        "=?base64?{}?=",
        crate::clipboard::base64_encode(text.as_bytes())
    )
}

/// The keywords whose subschemas a `properties` chain may **not** pass
/// through — an annotation under any of them makes the tool invalid (the
/// spec's "statically reachable" rule).
const UNREACHABLE_KEYWORDS: [&str; 18] = [
    "items",
    "prefixItems",
    "contains",
    "additionalProperties",
    "unevaluatedProperties",
    "unevaluatedItems",
    "propertyNames",
    "patternProperties",
    "dependentSchemas",
    "oneOf",
    "anyOf",
    "allOf",
    "not",
    "if",
    "then",
    "else",
    "$defs",
    "definitions",
];

/// Of those, the ones whose value maps names to subschemas.
const NAME_MAP_KEYWORDS: [&str; 4] = [
    "patternProperties",
    "dependentSchemas",
    "$defs",
    "definitions",
];

/// Every `x-mcp-header` declaration in a tool's input schema — or why the
/// tool must be refused: the first rule an annotation breaks.
pub fn param_headers(schema: &Value) -> Result<Vec<ParamHeader>, String> {
    let mut found = Vec::new();
    visit(schema, &mut Vec::new(), true, &mut found)?;
    Ok(found)
}

fn visit(
    node: &Value,
    path: &mut Vec<String>,
    reachable: bool,
    found: &mut Vec<ParamHeader>,
) -> Result<(), String> {
    let Some(schema) = node.as_object() else {
        return Ok(());
    };
    if let Some(raw) = schema.get(X_MCP_HEADER_KEY) {
        let at = if path.is_empty() {
            "the schema root".to_string()
        } else {
            format!("\"{}\"", path.join("."))
        };
        if !reachable || path.is_empty() {
            return Err(format!(
                "its x-mcp-header at {at} is outside a chain of properties"
            ));
        }
        let Some(name) = raw.as_str().filter(|name| !name.is_empty()) else {
            return Err(format!(
                "its x-mcp-header at {at} is not a non-empty string"
            ));
        };
        if !name.bytes().all(is_tchar) {
            return Err(format!(
                "its x-mcp-header \"{name}\" at {at} is not a valid header name"
            ));
        }
        let kind = schema.get("type").and_then(Value::as_str);
        if !matches!(kind, Some("string" | "integer" | "boolean")) {
            return Err(format!(
                "its x-mcp-header \"{name}\" is on {at}, typed {}, not string, integer or boolean",
                kind.unwrap_or("nothing")
            ));
        }
        if let Some(prior) = found
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
        {
            return Err(format!(
                "its x-mcp-header \"{name}\" repeats \"{}\"",
                prior.name
            ));
        }
        found.push(ParamHeader {
            path: path.clone(),
            name: name.to_string(),
        });
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (key, child) in properties {
            path.push(key.clone());
            let outcome = visit(child, path, reachable, found);
            path.pop();
            outcome?;
        }
    }
    for keyword in UNREACHABLE_KEYWORDS {
        let Some(sub) = schema.get(keyword) else {
            continue;
        };
        let branches: Vec<&Value> = match sub {
            Value::Array(items) => items.iter().collect(),
            Value::Object(map) if NAME_MAP_KEYWORDS.contains(&keyword) => map.values().collect(),
            other => vec![other],
        };
        for branch in branches {
            path.push(format!("<{keyword}>"));
            let outcome = visit(branch, path, false, found);
            path.pop();
            outcome?;
        }
    }
    Ok(())
}

/// RFC 9110 §5.6.2 `tchar` — the bytes a header field name may use.
const fn is_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// The `Mcp-Param-{Name}` headers one call carries: each declaration's
/// value read at its exact path in the arguments, as the spec converts it —
/// a string as-is, an integer in decimal, a boolean as `true`/`false` —
/// encoded by [`header_value`]. A value that is absent or `null` sends no
/// header (the spec's "MUST omit"); one that is not a primitive, or an
/// integer outside JavaScript's safe range, sends none either, rather than a
/// malformed one — the server's own argument validation answers that call.
#[must_use]
pub fn param_header_values(headers: &[ParamHeader], arguments: &Value) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|header| {
            let mut value = arguments;
            for key in &header.path {
                value = value.get(key)?;
            }
            let text = match value {
                Value::String(text) => text.clone(),
                Value::Bool(flag) => flag.to_string(),
                Value::Number(number) => number_text(number)?,
                _ => return None,
            };
            Some((
                format!("{MCP_PARAM_HEADER_PREFIX}{}", header.name),
                header_value(&text),
            ))
        })
        .collect()
}

/// A number as the header carries it — `None` past JavaScript's safe-integer
/// range (2^53 − 1) or for a value that isn't finite.
fn number_text(number: &serde_json::Number) -> Option<String> {
    const SAFE: u64 = (1 << 53) - 1;
    if let Some(n) = number.as_u64() {
        return (n <= SAFE).then(|| n.to_string());
    }
    if let Some(n) = number.as_i64() {
        return (n.unsigned_abs() <= SAFE).then(|| n.to_string());
    }
    let f = number.as_f64().filter(|f| f.is_finite())?;
    if f.fract() == 0.0 {
        return (f.abs() <= SAFE as f64).then(|| format!("{f:.0}"));
    }
    Some(f.to_string())
}

/// `schema` with every `x-mcp-header` annotation removed — the model's copy
/// carries no transport metadata (and an API that refuses unknown keywords
/// never meets one). Schema-aware: a *property named* `x-mcp-header` is a
/// name, not an annotation, and stays; instance data under `enum`, `const`,
/// `default` and `examples` is left alone.
#[must_use]
pub fn strip_header_annotations(schema: &Value) -> Value {
    let Some(map) = schema.as_object() else {
        return schema.clone();
    };
    let mut out = serde_json::Map::with_capacity(map.len());
    for (key, value) in map {
        if key == X_MCP_HEADER_KEY {
            continue;
        }
        let value = match key.as_str() {
            "enum" | "const" | "default" | "examples" => value.clone(),
            "properties" | "patternProperties" | "dependentSchemas" | "$defs" | "definitions" => {
                match value {
                    Value::Object(names) => Value::Object(
                        names
                            .iter()
                            .map(|(name, sub)| (name.clone(), strip_header_annotations(sub)))
                            .collect(),
                    ),
                    other => other.clone(),
                }
            }
            _ => match value {
                Value::Array(items) => {
                    Value::Array(items.iter().map(strip_header_annotations).collect())
                }
                Value::Object(_) => strip_header_annotations(value),
                other => other.clone(),
            },
        };
        out.insert(key.clone(), value);
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn header_values_encode_only_when_unsafe() {
        assert_eq!(header_value("get_weather"), "get_weather");
        assert_eq!(header_value("tools/call"), "tools/call");
        // Non-ASCII, whitespace-framed, and sentinel-looking values take the
        // spec's base64 sentinel form — the spec's own examples.
        assert_eq!(
            header_value("Hello, 世界"),
            "=?base64?SGVsbG8sIOS4lueVjA==?="
        );
        assert_eq!(header_value(" padded "), "=?base64?IHBhZGRlZCA=?=");
        assert_eq!(header_value("line1\nline2"), "=?base64?bGluZTEKbGluZTI=?=");
        assert_eq!(
            header_value("=?base64?literal?="),
            "=?base64?PT9iYXNlNjQ/bGl0ZXJhbD89?="
        );
        // Only the exact sentinel shape is ambiguous; `=?…?=` alone is not.
        assert_eq!(header_value("=?utf-8?q?x?="), "=?utf-8?q?x?=");
        assert!(header_value("").starts_with("=?base64?"));
    }

    /// The spec's own example: `region` is marked, `query` is not.
    #[test]
    fn an_annotated_parameter_is_declared_with_its_path() {
        let schema = json!({
            "type": "object",
            "properties": {
                "region": {"type": "string", "x-mcp-header": "Region"},
                "query": {"type": "string"},
                "target": {
                    "type": "object",
                    "properties": {"tenant": {"type": "integer", "x-mcp-header": "Tenant"}}
                }
            }
        });
        assert_eq!(
            param_headers(&schema),
            Ok(vec![
                ParamHeader {
                    path: vec!["region".into()],
                    name: "Region".into()
                },
                ParamHeader {
                    path: vec!["target".into(), "tenant".into()],
                    name: "Tenant".into()
                },
            ])
        );
        assert_eq!(param_headers(&json!({"type": "object"})), Ok(Vec::new()));
    }

    /// Every rule an annotation can break refuses the tool, with the reason.
    #[test]
    fn a_broken_annotation_refuses_the_tool() {
        let prop = |annotation: Value, kind: Value| json!({"type": "object", "properties": {"p": {"type": kind, "x-mcp-header": annotation}}});
        for (schema, says) in [
            (prop(json!(""), json!("string")), "non-empty"),
            (prop(json!(7), json!("string")), "non-empty"),
            (
                prop(json!("Bad Name"), json!("string")),
                "valid header name",
            ),
            (prop(json!("N"), json!("number")), "number"),
            (prop(json!("N"), json!("object")), "object"),
            (prop(json!("N"), json!(["string", "null"])), "nothing"),
            (
                json!({"type": "object", "x-mcp-header": "Root"}),
                "outside a chain",
            ),
            (
                json!({"type": "object", "properties": {"list": {"type": "array",
                    "items": {"type": "string", "x-mcp-header": "Item"}}}}),
                "outside a chain",
            ),
            (
                json!({"type": "object", "oneOf": [{"properties": {"a": {"type": "string",
                    "x-mcp-header": "A"}}}]}),
                "outside a chain",
            ),
            (
                json!({"type": "object", "properties": {
                    "a": {"type": "string", "x-mcp-header": "Region"},
                    "b": {"type": "string", "x-mcp-header": "region"}}}),
                "repeats",
            ),
        ] {
            let refused = param_headers(&schema).expect_err("refused");
            assert!(refused.contains(says), "{schema}: {refused}");
        }
    }

    #[test]
    fn values_are_mirrored_as_the_spec_converts_them() {
        let headers = vec![
            ParamHeader {
                path: vec!["region".into()],
                name: "Region".into(),
            },
            ParamHeader {
                path: vec!["n".into()],
                name: "N".into(),
            },
            ParamHeader {
                path: vec!["flag".into()],
                name: "Flag".into(),
            },
            ParamHeader {
                path: vec!["target".into(), "tenant".into()],
                name: "Tenant".into(),
            },
            ParamHeader {
                path: vec!["absent".into()],
                name: "Absent".into(),
            },
            ParamHeader {
                path: vec!["nothing".into()],
                name: "Nothing".into(),
            },
            ParamHeader {
                path: vec!["list".into()],
                name: "List".into(),
            },
            ParamHeader {
                path: vec!["huge".into()],
                name: "Huge".into(),
            },
        ];
        let arguments = json!({
            "region": "us-west1",
            "n": -7,
            "flag": true,
            "target": {"tenant": "Zürich"},
            "nothing": null,
            "list": [1],
            "huge": 9_007_199_254_740_993_u64
        });
        assert_eq!(
            param_header_values(&headers, &arguments),
            vec![
                ("Mcp-Param-Region".to_string(), "us-west1".to_string()),
                ("Mcp-Param-N".to_string(), "-7".to_string()),
                ("Mcp-Param-Flag".to_string(), "true".to_string()),
                (
                    "Mcp-Param-Tenant".to_string(),
                    "=?base64?WsO8cmljaA==?=".to_string()
                ),
            ]
        );
    }

    #[test]
    fn the_model_copy_carries_no_annotation_but_keeps_a_property_of_that_name() {
        let schema = json!({
            "type": "object",
            "properties": {
                "region": {"type": "string", "x-mcp-header": "Region"},
                "x-mcp-header": {"type": "string", "enum": [{"x-mcp-header": "data"}]}
            }
        });
        assert_eq!(
            strip_header_annotations(&schema),
            json!({
                "type": "object",
                "properties": {
                    "region": {"type": "string"},
                    "x-mcp-header": {"type": "string", "enum": [{"x-mcp-header": "data"}]}
                }
            })
        );
    }
}
