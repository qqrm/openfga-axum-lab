use serde_json::Value;
use sha2::{Digest, Sha256};

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn isolated_openlane_model_is_an_attributed_modified_subset() {
    let bytes = include_bytes!("../scenarios/openlane/fga/model.json");
    let model: Value = serde_json::from_slice(bytes).unwrap();
    let provenance: Value =
        serde_json::from_str(include_str!("../scenarios/openlane/fga/provenance.json")).unwrap();
    assert_eq!(provenance["sha256"], sha256(bytes));
    assert_eq!(provenance["project"], "Openlane");
    assert_eq!(
        provenance["repository"],
        "https://github.com/theopenlane/core"
    );
    assert_eq!(
        provenance["commit"],
        "1c38df0a53530c8c39eb0784862b06eaa3e26f58"
    );
    assert_eq!(provenance["model_changes"], true);
    assert_eq!(provenance["upstream_schema_version"], "1.2");
    assert_eq!(provenance["schema_version"], model["schema_version"]);
    assert_eq!(model["schema_version"], "1.1");
    assert_eq!(provenance["license"], "Apache-2.0");
    let mut types: Vec<_> = model["type_definitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|definition| definition["type"].as_str().unwrap())
        .collect();
    types.sort();
    assert_eq!(types, ["organization", "task", "user"]);
    let paths = provenance["source_paths"].as_array().unwrap();
    for source in [
        "fga/model/fga.mod",
        "fga/model/base/organization.fga",
        "fga/model/base/task.fga",
        "fga/model/roles/roles.fga",
        "fga/model/generated/crud.fga",
    ] {
        assert!(
            paths.iter().any(|value| value == source),
            "missing upstream source {source}"
        );
    }
    assert!(
        include_str!("../scenarios/openlane/LICENSES/Apache-2.0-Openlane.txt")
            .contains("Apache License")
    );
    let notice = include_str!("../scenarios/openlane/NOTICE");
    assert!(notice.contains("theopenlane"));
    assert!(notice.contains("1c38df0a53530c8c39eb0784862b06eaa3e26f58"));
}

#[test]
fn default_lakekeeper_model_retains_exact_upstream_provenance_and_license() {
    let bytes = include_bytes!("../fga/model.json");
    let provenance: Value = serde_json::from_str(include_str!("../fga/provenance.json")).unwrap();
    assert_eq!(
        sha256(bytes),
        "a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d"
    );
    assert_eq!(provenance["sha256"], sha256(bytes));
    assert_eq!(provenance["project"], "Lakekeeper");
    assert_eq!(
        provenance["commit"],
        "b73d5f8083a690ca4f533559e6ef3cbc6fbc13d6"
    );
    assert_eq!(provenance["model_changes"], false);
    assert_eq!(provenance["license"], "Apache-2.0");
    assert!(include_str!("../fga/upstream/lakekeeper.LICENSE").contains("Apache License"));
    assert!(include_str!("../fga/upstream/lakekeeper.NOTICE").contains("Vakamo"));
}
