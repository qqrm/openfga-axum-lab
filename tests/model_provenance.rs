use serde_json::Value;
use sha2::{Digest, Sha256};

#[test]
fn lakekeeper_model_is_the_exact_attributed_upstream_snapshot() {
    let bytes = include_bytes!("../fga/model.json");
    let hash = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        hash,
        "a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d"
    );
    let provenance: Value = serde_json::from_str(include_str!("../fga/provenance.json")).unwrap();
    assert_eq!(provenance["sha256"], hash);
    assert_eq!(
        provenance["commit"],
        "b73d5f8083a690ca4f533559e6ef3cbc6fbc13d6"
    );
    assert_eq!(provenance["model_changes"], false);
    assert_eq!(provenance["license"], "Apache-2.0");
    assert!(include_str!("../fga/upstream/lakekeeper.LICENSE").contains("Apache License"));
    assert!(include_str!("../fga/upstream/lakekeeper.NOTICE").contains("Vakamo"));
}
