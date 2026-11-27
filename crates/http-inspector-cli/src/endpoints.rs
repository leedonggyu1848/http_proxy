//! `records endpoints`: the request and response structure of each endpoint,
//! aggregated by the same code as the app's Map detail pane, and an
//! `OpenAPI` 3.1 document derived from it.
//!
//! Only names, types, and counts are aggregated; query values, header
//! values, and body field values are never kept (see the analysis crate's
//! privacy invariant).

use std::collections::BTreeMap;

use http_inspector_analysis::endpoint_schema::{
    BodySchema, EndpointKey, EndpointSchema, EndpointSchemas, FieldShape, NO_CONTENT_TYPE,
    StringFormat, TypeCounts, ValueKind, observe_exchange,
};
use http_inspector_analysis::search::is_settled;
use http_inspector_analysis::url::{IdKind, PathSegmentPattern};
use http_inspector_analysis::url_map::{
    SortDirection, TreeFilter, TreeSortField, UrlMap, ViewNode, ViewNodeKey, build_view_roots,
};
use http_inspector_core::recording::PreviewBodies;
use http_inspector_exchange::{CaptureSource, Exchange};
use serde_json::{Map, Value, json};

use crate::error::{CliError, ErrorCode};

/// One endpoint: a path pattern after ID folding, and a method.
pub(crate) struct Endpoint {
    key: ViewNodeKey,
    method: String,
    schema: EndpointSchema,
}

/// Endpoints aggregated from a set of records.
pub(crate) struct Collected {
    endpoints: Vec<Endpoint>,
    records: usize,
    in_flight: usize,
}

/// Aggregates settled HTTP records the way the app's Map does: place each in
/// the URL map, describe its endpoint observation, then fold IDs and merge.
pub(crate) fn collect<'a>(records: impl Iterator<Item = &'a Exchange>) -> Collected {
    let mut map = UrlMap::new();
    let mut schemas = EndpointSchemas::new();
    let mut used = 0;
    let mut in_flight = 0;
    for record in records.filter(|record| record.source() != CaptureSource::Connect) {
        if !is_settled(record) {
            in_flight += 1;
            continue;
        }
        if let Some(node) = map.observe_exchange(record) {
            let key = EndpointKey {
                node,
                method: record.request().method().clone(),
            };
            schemas.record(&key, &observe_exchange(record, &PreviewBodies));
            used += 1;
        }
    }
    let roots = build_view_roots(
        &map,
        &TreeFilter::default(),
        TreeSortField::Name,
        SortDirection::Asc,
    );
    let mut endpoints = Vec::new();
    let mut pending: Vec<&ViewNode> = roots.iter().rev().collect();
    while let Some(node) = pending.pop() {
        for (method, schema) in schemas.merged(&node.raw_node_ids) {
            endpoints.push(Endpoint {
                key: node.key.clone(),
                method,
                schema,
            });
        }
        pending.extend(node.children.iter().rev());
    }
    Collected {
        endpoints,
        records: used,
        in_flight,
    }
}

fn path_text(key: &ViewNodeKey) -> String {
    if key.path.is_empty() {
        return "/".to_owned();
    }
    key.path.iter().fold(String::new(), |mut path, segment| {
        path.push('/');
        path.push_str(&segment.display_text());
        path
    })
}

fn counts<K: Ord>(map: &BTreeMap<K, u64>, label: impl Fn(&K) -> String) -> Value {
    Value::Object(
        map.iter()
            .map(|(key, count)| (label(key), json!(count)))
            .collect(),
    )
}

fn type_counts(types: &TypeCounts) -> Value {
    json!({
        "kinds": counts(types.kinds(), |kind| kind.label().to_owned()),
        "formats": counts(types.formats(), |format| format.label().to_owned()),
    })
}

fn shape(field: &FieldShape) -> Value {
    json!({
        "seen": field.seen(),
        "types": type_counts(field.types()),
        "fields": field
            .fields()
            .iter()
            .map(|(name, child)| (name.clone(), shape(child)))
            .collect::<Map<_, _>>(),
        "items": field.items().map(shape),
        "omitted_fields": field.omitted_fields(),
        "depth_limited": field.depth_limited(),
    })
}

fn body(schema: &BodySchema) -> Value {
    json!({
        "bodies": schema.bodies(),
        "structured": schema.structured_samples(),
        "empty": schema.empty(),
        "invalid": schema.invalid(),
        "unstructured": schema.unstructured(),
        "excluded": counts(schema.excluded(), |reason| reason.label().to_owned()),
        "fields": shape(schema.fields()),
        "parts": schema
            .parts()
            .iter()
            .map(|(name, part)| json!({
                "name": name,
                "seen": part.seen(),
                "files": part.files(),
                "content_types": counts(part.content_types(), Clone::clone),
            }))
            .collect::<Vec<_>>(),
    })
}

fn endpoint_json(endpoint: &Endpoint) -> Value {
    let schema = &endpoint.schema;
    json!({
        "url": endpoint.key.display_url(),
        "origin": endpoint.key.origin.to_string(),
        "path": path_text(&endpoint.key),
        "method": endpoint.method,
        "requests": schema.requests(),
        "path_ids": schema
            .path_ids()
            .iter()
            .map(|(position, kinds)| json!({
                "position": position,
                "kinds": counts(kinds, |kind| kind.label().to_owned()),
            }))
            .collect::<Vec<_>>(),
        "query": schema
            .query()
            .iter()
            .map(|(name, stats)| json!({
                "name": name,
                "seen": stats.seen(),
                "types": type_counts(stats.types()),
            }))
            .collect::<Vec<_>>(),
        "omitted_query": schema.omitted_query(),
        "headers": counts(schema.headers(), Clone::clone),
        "auth": counts(schema.auth(), |scheme| scheme.label().to_owned()),
        "requests_without_body": schema.requests_without_body(),
        "request_bodies": schema
            .request_bodies()
            .iter()
            .map(|(content_type, group)| {
                let mut value = body(group);
                insert(&mut value, "content_type", json!(content_type));
                value
            })
            .collect::<Vec<_>>(),
        "responses": schema
            .responses()
            .iter()
            .map(|(key, group)| {
                let mut value = body(group);
                insert(&mut value, "status", json!(key.status));
                insert(&mut value, "content_type", json!(key.content_type));
                value
            })
            .collect::<Vec<_>>(),
        "missing_responses": schema.missing_responses(),
        "excluded_bodies": schema.excluded_bodies(),
    })
}

fn insert(value: &mut Value, key: &str, field: Value) {
    if let Some(object) = value.as_object_mut() {
        object.insert(key.to_owned(), field);
    }
}

impl Collected {
    /// The `records endpoints` document.
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "endpoints": self.endpoints.iter().map(endpoint_json).collect::<Vec<_>>(),
            "records_analyzed": self.records,
            "records_in_flight": self.in_flight,
        })
    }

    /// An `OpenAPI` 3.1 document for the one origin the endpoints share.
    pub(crate) fn to_openapi(&self) -> Result<Value, CliError> {
        let mut origins: Vec<String> = self
            .endpoints
            .iter()
            .map(|endpoint| endpoint.key.origin.to_string())
            .collect();
        origins.sort_unstable();
        origins.dedup();
        if origins.len() > 1 {
            return Err(CliError::new(
                ErrorCode::Usage,
                format!(
                    "OpenAPI describes one server, but the records span {}",
                    origins.join(", ")
                ),
            )
            .with_hint("narrow the records with `--filter host:<host>`"));
        }
        let mut paths = Map::new();
        for endpoint in &self.endpoints {
            let (path, parameters) = openapi_path(&endpoint.key, &endpoint.schema);
            let item = paths.entry(path).or_insert_with(|| json!({}));
            if !parameters.is_empty() {
                insert(item, "parameters", Value::Array(parameters));
            }
            insert(
                item,
                &endpoint.method.to_ascii_lowercase(),
                operation(&endpoint.schema),
            );
        }
        let server = origins.first().cloned().unwrap_or_default();
        Ok(json!({
            "openapi": "3.1.0",
            "info": {
                "title": format!("{server} (observed)"),
                "version": "observed",
                "description": "Inferred by HTTP Inspector from captured traffic. Fields are \
                    listed as observed; whether the server requires them cannot be known from \
                    a capture, so no field is marked required.",
            },
            "servers": if server.is_empty() { json!([]) } else { json!([{ "url": server }]) },
            "paths": paths,
        }))
    }
}

/// The `OpenAPI` path template and its path parameters. Folded IDs become
/// `{id}`, `{id2}`, … so every parameter name is unique.
fn openapi_path(key: &ViewNodeKey, schema: &EndpointSchema) -> (String, Vec<Value>) {
    let mut path = String::new();
    let mut parameters = Vec::new();
    for (position, segment) in key.path.iter().enumerate() {
        path.push('/');
        match segment {
            PathSegmentPattern::Literal(_) => path.push_str(&segment.display_text()),
            PathSegmentPattern::FoldedId => {
                let name = match parameters.len() {
                    0 => "id".to_owned(),
                    count => format!("id{}", count + 1),
                };
                let integer = schema
                    .path_ids()
                    .get(&position)
                    .is_some_and(|kinds| kinds.keys().all(|kind| *kind == IdKind::Integer));
                let uuid = schema
                    .path_ids()
                    .get(&position)
                    .is_some_and(|kinds| kinds.keys().all(|kind| *kind == IdKind::Uuid));
                let schema = if integer {
                    json!({ "type": "integer" })
                } else if uuid {
                    json!({ "type": "string", "format": "uuid" })
                } else {
                    json!({ "type": "string" })
                };
                path.push('{');
                path.push_str(&name);
                path.push('}');
                parameters.push(json!({
                    "name": name,
                    "in": "path",
                    "required": true,
                    "schema": schema,
                }));
            }
        }
    }
    if path.is_empty() {
        path.push('/');
    }
    (path, parameters)
}

fn operation(schema: &EndpointSchema) -> Value {
    let parameters: Vec<Value> = schema
        .query()
        .iter()
        .map(|(name, stats)| {
            json!({
                "name": name,
                "in": "query",
                "required": false,
                "schema": scalar_schema(stats.types()),
                "x-observed-in": stats.seen(),
            })
        })
        .collect();
    let mut operation = json!({
        "x-observed-requests": schema.requests(),
        "responses": responses(schema),
    });
    if !parameters.is_empty() {
        insert(&mut operation, "parameters", Value::Array(parameters));
    }
    let content: Map<String, Value> = schema
        .request_bodies()
        .iter()
        .filter(|(content_type, _)| content_type.as_str() != NO_CONTENT_TYPE)
        .map(|(content_type, group)| {
            (
                content_type.clone(),
                json!({ "schema": body_schema(group) }),
            )
        })
        .collect();
    if !content.is_empty() {
        insert(&mut operation, "requestBody", json!({ "content": content }));
    }
    operation
}

fn responses(schema: &EndpointSchema) -> Value {
    let mut responses = Map::new();
    for (key, group) in schema.responses() {
        let entry = responses
            .entry(key.status.to_string())
            .or_insert_with(|| json!({ "description": "Observed response" }));
        if key.content_type != NO_CONTENT_TYPE {
            let content = entry
                .as_object_mut()
                .map(|entry| entry.entry("content").or_insert_with(|| json!({})));
            if let Some(content) = content {
                insert(
                    content,
                    &key.content_type,
                    json!({ "schema": body_schema(group) }),
                );
            }
        }
    }
    if responses.is_empty() {
        responses.insert(
            "default".to_owned(),
            json!({ "description": "No response was observed" }),
        );
    }
    Value::Object(responses)
}

/// JSON Schema of a body group, or the empty (any) schema when no body was
/// structured.
fn body_schema(group: &BodySchema) -> Value {
    if group.structured_samples() == 0 {
        return json!({});
    }
    field_schema(group.fields())
}

fn field_schema(field: &FieldShape) -> Value {
    let mut schema = scalar_schema(field.types());
    if field.types().count(ValueKind::Object) > 0 {
        let properties: Map<String, Value> = field
            .fields()
            .iter()
            .map(|(name, child)| (name.clone(), field_schema(child)))
            .collect();
        insert(&mut schema, "properties", Value::Object(properties));
    }
    if let Some(items) = field.items() {
        insert(&mut schema, "items", field_schema(items));
    }
    schema
}

/// JSON Schema `type` (and `format` when every string shares one) for the
/// observed value types.
fn scalar_schema(types: &TypeCounts) -> Value {
    let kinds: Vec<&str> = types
        .kinds()
        .keys()
        .map(|kind| match kind {
            ValueKind::Null => "null",
            ValueKind::Boolean => "boolean",
            ValueKind::Integer => "integer",
            ValueKind::Number => "number",
            ValueKind::String => "string",
            ValueKind::Object => "object",
            ValueKind::Array => "array",
        })
        .collect();
    let mut schema = match kinds.as_slice() {
        [] => json!({}),
        [one] => json!({ "type": one }),
        many => json!({ "type": many }),
    };
    let formats: Vec<&StringFormat> = types.formats().keys().collect();
    let format = match formats.as_slice() {
        [StringFormat::DateTime] => Some("date-time"),
        [StringFormat::Date] => Some("date"),
        [StringFormat::Email] => Some("email"),
        [StringFormat::Uuid] => Some("uuid"),
        [StringFormat::Url] => Some("uri"),
        _ => None,
    };
    if let Some(format) = format {
        insert(&mut schema, "format", json!(format));
    }
    schema
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "indexing a serde_json value by key returns null instead of panicking"
)]
mod tests {
    use std::error::Error;

    use http::header::CONTENT_TYPE;
    use http::{HeaderValue, Method, StatusCode};
    use http_inspector_exchange::{BodyLifecycle, ExchangeBuilder, ResponseFixture};

    use super::*;

    fn user(id: u32) -> Result<Exchange, Box<dyn Error>> {
        Ok(ExchangeBuilder::http(
            Method::GET,
            format!("http://api.example.test/users/{id}?verbose=1"),
        )?
        .respond(
            ResponseFixture::new(StatusCode::OK)
                .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                .body(
                    format!(r#"{{"id":{id},"email":"u{id}@example.test","tags":["a"]}}"#),
                    BodyLifecycle::Complete,
                ),
        )?)
    }

    #[test]
    fn folds_ids_and_describes_bodies_without_values() -> Result<(), Box<dyn Error>> {
        let records = (1..=5).map(user).collect::<Result<Vec<_>, _>>()?;
        let collected = collect(records.iter());
        let document = collected.to_json();
        let endpoints = document["endpoints"].as_array().ok_or("no endpoints")?;
        assert_eq!(endpoints.len(), 1, "{document}");
        let endpoint = &endpoints[0];
        assert_eq!(endpoint["path"], "/users/{id}");
        assert_eq!(endpoint["method"], "GET");
        assert_eq!(endpoint["requests"], 5);
        assert_eq!(endpoint["query"][0]["name"], "verbose");
        let fields = &endpoint["responses"][0]["fields"]["fields"];
        assert_eq!(fields["email"]["types"]["formats"]["email"], 5);
        assert!(!document.to_string().contains("u1@example.test"));

        let openapi = collected.to_openapi()?;
        let get = &openapi["paths"]["/users/{id}"]["get"];
        assert_eq!(openapi["servers"][0]["url"], "http://api.example.test");
        assert_eq!(
            openapi["paths"]["/users/{id}"]["parameters"][0]["schema"]["type"],
            "integer"
        );
        let schema = &get["responses"]["200"]["content"]["application/json"]["schema"];
        assert_eq!(schema["properties"]["email"]["format"], "email");
        assert_eq!(schema["properties"]["tags"]["items"]["type"], "string");
        assert!(schema.get("required").is_none());
        Ok(())
    }

    #[test]
    fn openapi_needs_a_single_origin() -> Result<(), Box<dyn Error>> {
        let other = ExchangeBuilder::http(Method::GET, "http://other.example.test/")?
            .respond(ResponseFixture::new(StatusCode::NO_CONTENT))?;
        let records = [user(1)?, other];
        let error = collect(records.iter())
            .to_openapi()
            .err()
            .map(|error| error.code);
        assert_eq!(error, Some(ErrorCode::Usage));
        Ok(())
    }
}
