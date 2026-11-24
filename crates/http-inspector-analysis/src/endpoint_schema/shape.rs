//! Field-structure aggregate for JSON and form bodies.
//!
//! A [`FieldShape`] keeps field names, observed types and how often each
//! field appeared. Values are classified and dropped while the shape is
//! built, so no original value can be read back from it.

use std::collections::BTreeMap;

use serde_json::Value;

use super::value::{TypeCounts, ValueKind, ValueShape};
use crate::url::classify_id_segment;

/// Deepest object/array nesting level that is described; deeper values are
/// counted by type only.
pub const MAX_SHAPE_DEPTH: usize = 8;
/// Most field entries kept in one body group, counting nested fields.
pub const MAX_SHAPE_FIELDS: usize = 256;
/// Longest field name kept, in bytes; longer names are shortened.
const MAX_FIELD_NAME_BYTES: usize = 64;
/// Name shown for object keys that look like generated identifiers.
pub const ID_KEY: &str = "{id}";

/// Observed structure of one value position (a body root, field, or array item).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FieldShape {
    seen: u64,
    types: TypeCounts,
    fields: BTreeMap<String, Self>,
    items: Option<Box<Self>>,
    omitted_fields: u64,
    depth_limited: bool,
}

impl FieldShape {
    /// Times a value was present at this position.
    #[must_use]
    pub const fn seen(&self) -> u64 {
        self.seen
    }

    /// Types and string formats observed at this position.
    #[must_use]
    pub const fn types(&self) -> &TypeCounts {
        &self.types
    }

    /// Fields observed when the value was an object, by name.
    #[must_use]
    pub const fn fields(&self) -> &BTreeMap<String, Self> {
        &self.fields
    }

    /// Element structure observed when the value was an array.
    #[must_use]
    pub fn items(&self) -> Option<&Self> {
        self.items.as_deref()
    }

    /// Number of objects observed here: the denominator for each field's ratio.
    #[must_use]
    pub fn object_count(&self) -> u64 {
        self.types.count(ValueKind::Object)
    }

    /// Fields not kept because the group reached [`MAX_SHAPE_FIELDS`].
    #[must_use]
    pub const fn omitted_fields(&self) -> u64 {
        self.omitted_fields
    }

    /// Reports whether nested values below [`MAX_SHAPE_DEPTH`] were not described.
    #[must_use]
    pub const fn depth_limited(&self) -> bool {
        self.depth_limited
    }

    /// Number of field and item entries in this subtree, excluding itself.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        let fields: usize = self
            .fields
            .values()
            .map(|field| 1 + field.entry_count())
            .sum();
        let items = self
            .items
            .as_ref()
            .map_or(0, |items| 1 + items.entry_count());
        fields.saturating_add(items)
    }

    /// Describes one parsed JSON document.
    #[must_use]
    pub fn from_json(value: &Value) -> Self {
        let mut budget = MAX_SHAPE_FIELDS;
        let mut shape = Self::default();
        shape.record_json(value, 0, &mut budget);
        shape
    }

    /// Describes one flat set of named text values such as a form body.
    #[must_use]
    pub fn from_text_fields<'a>(pairs: impl IntoIterator<Item = (String, &'a str)>) -> Self {
        let mut root = Self {
            seen: 1,
            ..Self::default()
        };
        root.types.record(ValueShape::plain(ValueKind::Object));
        let mut present: BTreeMap<String, Self> = BTreeMap::new();
        for (name, value) in pairs {
            let key = field_key(&name);
            if !present.contains_key(&key) && present.len() >= MAX_SHAPE_FIELDS {
                root.omitted_fields = root.omitted_fields.saturating_add(1);
                continue;
            }
            let field = present.entry(key).or_default();
            field.seen = 1;
            field.types.record(ValueShape::of_text(value));
        }
        root.fields = present;
        root
    }

    /// Adds every observation of `other` to this shape, keeping at most
    /// `budget` new entries.
    pub fn merge(&mut self, other: &Self, budget: &mut usize) {
        self.seen = self.seen.saturating_add(other.seen);
        self.types.merge(&other.types);
        self.omitted_fields = self.omitted_fields.saturating_add(other.omitted_fields);
        self.depth_limited |= other.depth_limited;
        for (name, field) in &other.fields {
            if let Some(existing) = self.fields.get_mut(name) {
                existing.merge(field, budget);
                continue;
            }
            let needed = 1 + field.entry_count();
            if needed > *budget {
                self.omitted_fields = self.omitted_fields.saturating_add(1);
                continue;
            }
            *budget -= needed;
            let _ = self.fields.insert(name.clone(), field.clone());
        }
        if let Some(items) = &other.items {
            if let Some(existing) = &mut self.items {
                existing.merge(items, budget);
            } else {
                let needed = 1 + items.entry_count();
                if needed > *budget {
                    self.omitted_fields = self.omitted_fields.saturating_add(1);
                } else {
                    *budget -= needed;
                    self.items = Some(items.clone());
                }
            }
        }
    }

    fn record_json(&mut self, value: &Value, depth: usize, budget: &mut usize) {
        self.seen = self.seen.saturating_add(1);
        let shape = match value {
            Value::Null => ValueShape::plain(ValueKind::Null),
            Value::Bool(_) => ValueShape::plain(ValueKind::Boolean),
            Value::Number(number) if number.is_i64() || number.is_u64() => {
                ValueShape::plain(ValueKind::Integer)
            }
            Value::Number(_) => ValueShape::plain(ValueKind::Number),
            Value::String(text) => ValueShape::of_json_string(text),
            Value::Array(_) => ValueShape::plain(ValueKind::Array),
            Value::Object(_) => ValueShape::plain(ValueKind::Object),
        };
        self.types.record(shape);
        match value {
            Value::Object(_) | Value::Array(_) if depth >= MAX_SHAPE_DEPTH => {
                self.depth_limited = true;
            }
            Value::Object(map) => self.record_object(map, depth, budget),
            Value::Array(values) => self.record_array(values, depth, budget),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }

    fn record_object(
        &mut self,
        map: &serde_json::Map<String, Value>,
        depth: usize,
        budget: &mut usize,
    ) {
        // A key repeated after ID folding (`{"1":…,"2":…}`) counts once per object.
        let mut folded: BTreeMap<String, Self> = BTreeMap::new();
        for (name, value) in map {
            let key = field_key(name);
            if !folded.contains_key(&key) && !self.fields.contains_key(&key) {
                if *budget == 0 {
                    self.omitted_fields = self.omitted_fields.saturating_add(1);
                    continue;
                }
                *budget -= 1;
            }
            let field = folded.entry(key).or_default();
            field.record_json(value, depth + 1, budget);
            field.seen = 1;
        }
        for (key, field) in folded {
            let mut unlimited = usize::MAX;
            self.fields
                .entry(key)
                .or_default()
                .merge(&field, &mut unlimited);
        }
    }

    fn record_array(&mut self, values: &[Value], depth: usize, budget: &mut usize) {
        if values.is_empty() {
            return;
        }
        if self.items.is_none() {
            if *budget == 0 {
                self.omitted_fields = self.omitted_fields.saturating_add(1);
                return;
            }
            *budget -= 1;
        }
        let items = self.items.get_or_insert_with(Box::default);
        for value in values {
            items.record_json(value, depth + 1, budget);
        }
    }
}

/// Bounds an external field name and folds generated identifiers into one key.
fn field_key(name: &str) -> String {
    if classify_id_segment(name).is_some() {
        return ID_KEY.to_owned();
    }
    if name.len() <= MAX_FIELD_NAME_BYTES {
        return name.to_owned();
    }
    let cut = name.floor_char_boundary(MAX_FIELD_NAME_BYTES);
    format!("{}…", name.get(..cut).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(text: &str) -> Result<FieldShape, serde_json::Error> {
        serde_json::from_str::<Value>(text).map(|value| FieldShape::from_json(&value))
    }

    #[test]
    fn merges_optional_null_and_mixed_fields_with_ratios() -> Result<(), serde_json::Error> {
        let mut total = json(r#"{"id":1,"name":"a","tags":["x"],"meta":null}"#)?;
        let mut budget = MAX_SHAPE_FIELDS - total.entry_count();
        total.merge(&json(r#"{"id":"u-1","meta":{"k":true}}"#)?, &mut budget);

        assert_eq!(total.seen(), 2);
        assert_eq!(total.object_count(), 2);
        let id = total.fields().get("id");
        assert_eq!(id.map(FieldShape::seen), Some(2));
        assert!(id.is_some_and(|field| field.types().is_mixed()));
        assert_eq!(total.fields().get("name").map(FieldShape::seen), Some(1));
        let meta = total.fields().get("meta");
        assert_eq!(
            meta.map(|field| field.types().summary()),
            Some("null | object".to_owned())
        );
        assert_eq!(
            meta.and_then(|field| field.fields().get("k"))
                .map(FieldShape::seen),
            Some(1)
        );
        let tags = total.fields().get("tags").and_then(FieldShape::items);
        assert_eq!(
            tags.map(|items| items.types().summary()),
            Some("string".to_owned())
        );
        Ok(())
    }

    #[test]
    fn never_keeps_values_and_folds_id_keys() -> Result<(), serde_json::Error> {
        let shape = json(r#"{"secret":"TOPSECRET_VALUE","12345":{"a":1},"67890":{"a":2}}"#)?;
        let debug = format!("{shape:?}");
        assert!(!debug.contains("TOPSECRET_VALUE"));
        assert!(!debug.contains("12345"));
        let ids = shape.fields().get(ID_KEY);
        assert_eq!(ids.map(FieldShape::seen), Some(1));
        // Both folded objects had `a`, so it appears in 2 of the 2 objects.
        assert_eq!(ids.map(FieldShape::object_count), Some(2));
        assert_eq!(
            ids.and_then(|field| field.fields().get("a"))
                .map(FieldShape::seen),
            Some(2)
        );
        Ok(())
    }

    #[test]
    fn limits_depth_and_field_count() -> Result<(), serde_json::Error> {
        let deep = json(r#"{"a":{"b":{"c":{"d":{"e":{"f":{"g":{"h":{"i":{"j":1}}}}}}}}}}"#)?;
        let mut cursor = Some(&deep);
        let mut levels = 0;
        while let Some(shape) = cursor {
            if shape.depth_limited() {
                break;
            }
            cursor = shape.fields().values().next();
            levels += 1;
        }
        assert_eq!(levels, MAX_SHAPE_DEPTH);

        let wide: String = (0..400)
            .map(|index| format!("\"f{index}\":1"))
            .collect::<Vec<_>>()
            .join(",");
        let shape = json(&format!("{{{wide}}}"))?;
        assert_eq!(shape.fields().len(), MAX_SHAPE_FIELDS);
        assert_eq!(shape.omitted_fields(), 400 - 256);
        Ok(())
    }

    #[test]
    fn text_fields_record_value_types_once_per_name() {
        let shape = FieldShape::from_text_fields([
            ("page".to_owned(), "2"),
            ("tag".to_owned(), "a"),
            ("tag".to_owned(), "b"),
        ]);
        assert_eq!(shape.object_count(), 1);
        assert_eq!(shape.fields().get("tag").map(FieldShape::seen), Some(1));
        assert_eq!(
            shape
                .fields()
                .get("page")
                .map(|field| field.types().summary()),
            Some("integer".to_owned())
        );
    }
}
