// Published method shapes are contracts, independent of any running server.
use super::operations::client_shell_method_names as supported_client_shell_method_names;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
fn collect_schema_refs(value: &serde_json::Value, refs: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(serde_json::Value::as_str) {
                if let Some(name) = reference.rsplit('/').next() {
                    refs.insert(name.to_owned());
                }
            }
            for value in object.values() {
                collect_schema_refs(value, refs);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_schema_refs(value, refs);
            }
        }
        _ => {}
    }
}

fn normalized_wire_schema(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .iter()
                .filter(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "description" | "examples" | "readOnly" | "title" | "writeOnly"
                    )
                })
                .map(|(key, value)| (key.clone(), normalized_wire_schema(value)))
                .collect(),
        ),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(normalized_wire_schema).collect())
        }
        _ => value.clone(),
    }
}

fn endpoint_method_shape_digests() -> BTreeMap<String, String> {
    let schema = serde_json::to_value(schemars::schema_for!(crate::api::schema::Request))
        .expect("request schema");
    let definitions = schema
        .get("$defs")
        .and_then(serde_json::Value::as_object)
        .expect("request definitions");
    let branches = schema
        .get("oneOf")
        .and_then(serde_json::Value::as_array)
        .expect("request method branches");
    let mut digests = BTreeMap::new();

    for method in supported_client_shell_method_names() {
        let branch = branches
            .iter()
            .find(|branch| {
                branch
                    .pointer("/properties/method/const")
                    .and_then(serde_json::Value::as_str)
                    == Some(method)
            })
            .unwrap_or_else(|| panic!("missing request schema branch for {method}"));
        let mut referenced_names = BTreeSet::new();
        collect_schema_refs(branch, &mut referenced_names);
        let mut visited_names = BTreeSet::new();
        let mut selected_definitions = serde_json::Map::new();
        while let Some(name) = referenced_names.pop_first() {
            if !visited_names.insert(name.clone()) {
                continue;
            }
            let definition = definitions
                .get(&name)
                .unwrap_or_else(|| panic!("missing schema definition {name} for {method}"));
            collect_schema_refs(definition, &mut referenced_names);
            selected_definitions.insert(name, normalized_wire_schema(definition));
        }
        let shape = serde_json::json!({
            "request": normalized_wire_schema(branch),
            "definitions": selected_definitions,
        });
        let bytes = serde_json::to_vec(&shape).expect("method shape json");
        digests.insert(method.to_string(), format!("{:x}", Sha256::digest(bytes)));
    }

    digests
}

#[test]
fn advertised_client_shell_method_shapes_stay_at_the_v1_contract() {
    let expected: BTreeMap<String, String> = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/endpoint-method-shapes-v1.json"
    )))
    .expect("endpoint method shape fixture");
    let mut actual = endpoint_method_shape_digests();
    // Freeze additive methods separately without rewriting the published fixture.
    assert_eq!(
        actual.remove("pane.clear").as_deref(),
        Some("0301d288ba198ddaa427dd7421c71911cccaf4ea03544531efa8b67ca21b08f6")
    );
    assert_eq!(
        actual.remove("pane.link.resolve").as_deref(),
        Some("f5e4a3e01453ae7b188f127ce951c12c20e0bebcc17cc364eeb6d1a01fd5bf81")
    );

    assert_eq!(
            actual, expected,
            "an existing endpoint method changed shape; add load-bearing behavior as a new advertised method or explicitly gate new fields"
        );
}
