//! JSON Schema sanitization for the Gemini Function Declaration proto.
//!
//! Gemini's API uses a proto3 `Schema` message where `type` is a scalar enum
//! field. OpenAI's JSON Schema extension allows `"type": ["X", "null"]` to
//! represent nullable types, but proto3 scalars cannot start a JSON array —
//! Gemini rejects the request with "Proto field is not repeating, cannot
//! start list."
//!
//! This module recursively:
//!   1. Collapses `"type": ["X", "null"]` → `"type": "X"` (picks the
//!      non-null type).
//!   2. Removes OpenAI-only fields (`additionalProperties`, `strict`) that
//!      Gemini does not recognize and would cause unknown-field errors.

use serde_json::Value;

/// Normalize a JSON Schema `Value` so it is compatible with Gemini's
/// proto-backed function declaration schema. See module docs for details.
pub(crate) fn normalize_gemini_schema(schema: &mut Value) {
    let obj = match schema.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // 1. Strip OpenAI-only top-level fields.
    obj.remove("additionalProperties");
    obj.remove("strict");

    // 2. Collapse `"type": ["primary", "null"]` → `"type": "primary"`.
    if let Some(type_val) = obj.get("type") {
        if let Some(arr) = type_val.as_array() {
            // Pick the first non-"null" element as the canonical type.
            let primary = arr
                .iter()
                .find(|v| v.as_str().is_none_or(|s| s != "null"))
                .or_else(|| arr.first())
                .cloned();
            if let Some(canonical) = primary {
                obj.insert("type".to_string(), canonical);
            }
        }
    }

    // 3. Recurse into nested schemas (properties values, items, etc.).
    let keys: Vec<String> = obj.keys().cloned().collect();
    for key in keys {
        if let Some(child) = obj.get_mut(&key) {
            if key == "properties" {
                // properties is an object whose values are sub-schemas.
                if let Some(props) = child.as_object_mut() {
                    for prop_schema in props.values_mut() {
                        normalize_gemini_schema(prop_schema);
                    }
                }
            } else if matches!(key.as_str(), "items" | "additionalItems" | "not") {
                normalize_gemini_schema(child);
            } else if matches!(key.as_str(), "allOf" | "anyOf" | "oneOf") {
                if let Some(arr) = child.as_array_mut() {
                    for sub in arr.iter_mut() {
                        normalize_gemini_schema(sub);
                    }
                }
            }
        }
    }
}
