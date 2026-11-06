//! Fail-closed release policy. Files are supplied by authenticated GitHub CLI calls.
use crate::ToolResult;
use serde_json::Value;

fn hex(value: &str, len: usize) -> bool { value.len() == len && value.bytes().all(|c| c.is_ascii_hexdigit()) }
pub fn tag_valid(tag: &str) -> bool {
    let Some(tag) = tag.strip_prefix('v') else { return false; };
    let (version, rc) = tag.split_once("-rc.").map_or((tag, None), |(v, r)| (v, Some(r)));
    let parts: Vec<_> = version.split('.').collect();
    let number = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()) && (s.len() == 1 || !s.starts_with('0'));
    parts.len() == 3 && parts.into_iter().all(number) && rc.is_none_or(number)
}
fn digest(image: &str, repositories: &[&str]) -> bool {
    image.split_once("@sha256:").is_some_and(|(repo, sha)| repositories.contains(&repo) && hex(sha, 64))
}
fn manifest(value: &Value, tag: &str) -> ToolResult {
    let revision = value["revision"].as_str().ok_or("missing revision")?;
    if value["schema"] != 1 || value["tag"] != tag || !tag_valid(tag) || !hex(revision, 40)
        || !digest(value["image"].as_str().unwrap_or(""), &["ghcr.io/qqrm/openfga-axum-lab"])
        || !digest(value["postgres_image"].as_str().unwrap_or(""), &["postgres", "docker.io/library/postgres"])
        || !digest(value["openfga_image"].as_str().unwrap_or(""), &["openfga/openfga", "docker.io/openfga/openfga"]) {
        return Err("release manifest identity or immutable image validation failed".into());
    }
    Ok(())
}
fn runs(value: &Value, sha: &str) -> ToolResult {
    if !hex(sha, 40) { return Err("invalid source SHA".into()); }
    let pages: Vec<&Value> = value.as_array().map_or_else(|| vec![value], |pages| pages.iter().collect());
    let entries: Vec<&Value> = pages.into_iter().filter_map(|p| p["workflow_runs"].as_array()).flatten().collect();
    for workflow in ["ci.yml", "workflow-security.yml", "secrets.yml", "delivery.yml"] {
        let path = format!(".github/workflows/{workflow}");
        let latest = entries.iter().filter(|r| r["head_sha"] == sha && r["head_branch"] == "main"
            && r["path"] == path && matches!(r["event"].as_str(), Some("push" | "workflow_dispatch")))
            .max_by_key(|r| r["id"].as_u64().unwrap_or(0)).ok_or_else(|| format!("missing exact-SHA main verification: {workflow}"))?;
        if latest["status"] != "completed" || latest["conclusion"] != "success" {
            return Err(format!("latest exact-SHA check is not successful: {workflow}").into());
        }
    }
    Ok(())
}
fn environment(value: &Value, name: &str) -> ToolResult {
    if !matches!(name, "staging" | "stable" | "release") || value["name"] != name
        || value["deployment_branch_policy"]["protected_branches"] != true {
        return Err("environment must exist and restrict deployments to protected branches".into());
    }
    let reviewed = value["protection_rules"].as_array().is_some_and(|rules| rules.iter().any(|r|
        r["type"] == "required_reviewers" && r["reviewers"].as_array().is_some_and(|a| !a.is_empty())));
    if name != "staging" && !reviewed { return Err("stable/release must have required reviewers configured".into()); }
    Ok(())
}
fn receipt(value: &Value, manifest: &Value) -> ToolResult {
    if value["success"] != true || value["environment"] != "staging"
        || ["image", "revision", "tag"].iter().any(|k| value[k] != manifest[k])
        || value["observed_revision"] != manifest["revision"] {
        return Err("no successful staging receipt for this exact candidate".into());
    }
    Ok(())
}
pub fn run(args: &[String]) -> ToolResult {
    let read = |p: &str| -> ToolResult<Value> { Ok(serde_json::from_slice(&std::fs::read(p)?)?) };
    match args {
        [action, path, sha] if action == "runs" => runs(&read(path)?, sha),
        [action, path, tag] if action == "manifest" => manifest(&read(path)?, tag),
        [action, path, name] if action == "environment" => environment(&read(path)?, name),
        [action, path, candidate] if action == "receipt" => receipt(&read(path)?, &read(candidate)?),
        [action, tag] if action == "tag" && tag_valid(tag) => Ok(()),
        _ => Err("usage: lab-tool delivery-policy <tag TAG|runs FILE SHA|manifest FILE TAG|environment FILE NAME|receipt FILE MANIFEST>".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn candidate() -> Value { json!({"schema":1,"tag":"v0.1.0-rc.1","revision":"a".repeat(40),
        "image":format!("ghcr.io/qqrm/openfga-axum-lab@sha256:{}", "b".repeat(64)),
        "postgres_image":format!("postgres@sha256:{}", "c".repeat(64)),
        "openfga_image":format!("openfga/openfga@sha256:{}", "d".repeat(64))}) }
    #[test]
    fn strict_release_names_and_images() {
        for tag in ["v0.1.0", "v1.2.3-rc.1"] { assert!(tag_valid(tag)); }
        for tag in ["main", "v01.1.0", "v1.2.3;exit", "v1.2.3-rc.", "v1.2.3/xx"] { assert!(!tag_valid(tag)); }
        let mut value = candidate(); assert!(manifest(&value, "v0.1.0-rc.1").is_ok());
        value["image"] = json!("ghcr.io/qqrm/openfga-axum-lab:latest");
        assert!(manifest(&value, "v0.1.0-rc.1").is_err());
    }
    #[test]
    fn older_success_does_not_hide_new_failure_or_wrong_commit() {
        let sha = "a".repeat(40);
        let mut entries: Vec<Value> = ["ci.yml","workflow-security.yml","secrets.yml","delivery.yml"].iter().map(|w|
            json!({"id":1,"path":format!(".github/workflows/{w}"),"head_sha":sha,"head_branch":"main","event":"push","status":"completed","conclusion":"success"})).collect();
        assert!(runs(&json!({"workflow_runs":entries}), &sha).is_ok());
        let mut failed = entries[0].clone(); failed["id"] = json!(2); failed["conclusion"] = json!("failure"); entries.push(failed);
        assert!(runs(&json!({"workflow_runs":entries}), &sha).is_err());
        assert!(runs(&json!({"workflow_runs":entries}), &"e".repeat(40)).is_err());
    }
    #[test]
    fn stable_requires_real_review_policy_and_matching_receipt() {
        let mut env = json!({"name":"stable","deployment_branch_policy":{"protected_branches":true},"protection_rules":[]});
        assert!(environment(&env, "stable").is_err());
        env["protection_rules"] = json!([{"type":"required_reviewers","reviewers":[{"id":1}]}]);
        assert!(environment(&env, "stable").is_ok());
        let manifest = candidate(); let mut proof = manifest.clone();
        proof["success"] = json!(true); proof["environment"] = json!("staging"); proof["observed_revision"] = manifest["revision"].clone();
        assert!(receipt(&proof, &manifest).is_ok());
        proof["image"] = json!("different"); assert!(receipt(&proof, &manifest).is_err());
    }
}
