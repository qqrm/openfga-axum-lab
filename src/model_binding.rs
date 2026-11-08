//! Semantic comparison of a pinned OpenFGA model with its HTTP representation.
//!
//! OpenFGA's protobuf gateway emits empty defaults that a compiled model file
//! may omit. This comparison normalizes those defaults and documentation only;
//! relation rewrites, subject restrictions, wildcards and conditions remain.
//! Store/model identifiers are checked separately by each caller. The original
//! model bytes and their digest are never rewritten by this module.
use serde_json::{Value, json};
use std::collections::BTreeSet;

type BindingResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn normalize_rewrite(value: &mut Value) {
    match value {
        Value::Array(children) => children.iter_mut().for_each(normalize_rewrite),
        Value::Object(fields) => {
            for (name, child) in fields {
                if matches!(name.as_str(), "computedUserset" | "tupleset")
                    && let Some(fields) = child.as_object_mut()
                    && fields.get("object").and_then(Value::as_str) == Some("")
                {
                    fields.remove("object");
                }
                normalize_rewrite(child);
            }
        }
        _ => {}
    }
}

/// Canonicalize only protobuf defaults and documentation metadata for comparison.
/// This does not evaluate, migrate, weaken or validate a model for publication.
pub fn policy_model(model: &Value) -> BindingResult<Value> {
    let definitions = model["type_definitions"]
        .as_array()
        .ok_or("missing model types")?;
    if definitions.is_empty() {
        return Err("model has no types".into());
    }
    let schema = model["schema_version"]
        .as_str()
        .ok_or("missing model schema")?;
    let mut seen = BTreeSet::new();
    let mut types = Vec::new();
    for definition in definitions {
        let name = definition["type"]
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or("missing model type name")?;
        if !seen.insert(name) {
            return Err("model repeats a type name".into());
        }
        let mut relations = definition.get("relations").cloned().unwrap_or(json!({}));
        if !relations.is_object() {
            return Err("invalid model relations".into());
        }
        normalize_rewrite(&mut relations);
        let mut metadata = definition["metadata"]
            .get("relations")
            .cloned()
            .unwrap_or(json!({}));
        let metadata_relations = metadata
            .as_object_mut()
            .ok_or("invalid relation metadata")?;
        for metadata in metadata_relations.values_mut() {
            let fields = metadata
                .as_object_mut()
                .ok_or("invalid relation metadata")?;
            fields.remove("module");
            fields.remove("source_info");
            let subjects = fields
                .entry("directly_related_user_types")
                .or_insert(json!([]))
                .as_array_mut()
                .ok_or("invalid related subject types")?;
            for subject in subjects {
                let fields = subject
                    .as_object_mut()
                    .ok_or("invalid subject restriction")?;
                if fields.get("condition").and_then(Value::as_str) == Some("") {
                    fields.remove("condition");
                }
            }
        }
        types.push(json!({"type":name,"relations":relations,"metadata":{"relations":metadata}}));
    }
    types.sort_by(|left, right| left["type"].as_str().cmp(&right["type"].as_str()));
    let conditions = model
        .get("conditions")
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or(json!({}));
    if !conditions.is_object() {
        return Err("invalid model conditions".into());
    }
    Ok(json!({"schema_version":schema,"type_definitions":types,"conditions":conditions}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Value {
        serde_json::from_slice(include_bytes!("../fga/model.json")).unwrap()
    }
    fn named<'a>(model: &'a mut Value, name: &str) -> &'a mut Value {
        model["type_definitions"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|value| value["type"] == name)
            .unwrap()
    }
    fn gateway_defaults(value: &mut Value) {
        match value {
            Value::Array(children) => children.iter_mut().for_each(gateway_defaults),
            Value::Object(fields) => {
                for (key, value) in fields {
                    if matches!(key.as_str(), "computedUserset" | "tupleset") {
                        value["object"] = json!("");
                    }
                    gateway_defaults(value);
                }
            }
            _ => {}
        }
    }
    #[test]
    fn gateway_defaults_and_documentation_do_not_change_policy_binding() {
        let original = model();
        let mut gateway = original.clone();
        gateway["id"] = json!("01ARZ3NDEKTSV4RRFFQ69G5FAV");
        gateway["conditions"] = Value::Null;
        for definition in gateway["type_definitions"].as_array_mut().unwrap() {
            if let Some(relations) = definition.get_mut("relations") {
                gateway_defaults(relations);
            }
            if let Some(relations) = definition["metadata"]
                .get_mut("relations")
                .and_then(Value::as_object_mut)
            {
                for metadata in relations.values_mut() {
                    metadata["module"] = json!("documentation-only");
                    metadata["source_info"] = json!({"file":"catalog.fga"});
                    let subjects = metadata
                        .as_object_mut()
                        .unwrap()
                        .entry("directly_related_user_types")
                        .or_insert(json!([]))
                        .as_array_mut()
                        .unwrap();
                    for subject in subjects {
                        subject["condition"] = json!("");
                    }
                }
            }
        }
        gateway["type_definitions"]
            .as_array_mut()
            .unwrap()
            .reverse();
        assert_eq!(
            policy_model(&original).unwrap(),
            policy_model(&gateway).unwrap()
        );
    }
    #[test]
    fn graph_subject_condition_and_wildcard_changes_remain_visible() {
        let original = model();
        let expected = policy_model(&original).unwrap();
        let mut changed = original.clone();
        named(&mut changed, "task")["relations"]["can_view"] = json!({"this":{}});
        assert_ne!(expected, policy_model(&changed).unwrap());
        let mut changed = original.clone();
        named(&mut changed, "task")["metadata"]["relations"]["viewer"]["directly_related_user_types"]
            [0]["wildcard"] = json!({});
        assert_ne!(expected, policy_model(&changed).unwrap());
        let mut changed = original.clone();
        named(&mut changed, "task")["metadata"]["relations"]["viewer"]["directly_related_user_types"]
            [0]["condition"] = json!("requires_mfa");
        assert_ne!(expected, policy_model(&changed).unwrap());
        let mut changed = original.clone();
        named(&mut changed, "task")["relations"]["parent_viewer"]["tupleToUserset"]["computedUserset"]
            ["object"] = json!("organization:other");
        assert_ne!(expected, policy_model(&changed).unwrap());
        let mut changed = original;
        changed["conditions"] =
            json!({"requires_mfa":{"name":"requires_mfa","expression":"false","parameters":{}}});
        assert_ne!(expected, policy_model(&changed).unwrap());
    }
    #[test]
    fn duplicate_types_and_malformed_structures_are_rejected() {
        let mut changed = model();
        let duplicate = changed["type_definitions"][0].clone();
        changed["type_definitions"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(policy_model(&changed).is_err());
        assert!(policy_model(&json!({"schema_version":"1.1","type_definitions":[]})).is_err());
    }
}
