use crate::ToolResult;
use serde_json::Value;
use std::{fs, process::Command};

fn verified_bot_commits(commits: &[Value], head: &str) -> bool {
    !head.is_empty()
        && !commits.is_empty()
        && commits.last().and_then(|item| item["sha"].as_str()) == Some(head)
        && commits.iter().all(|item| {
            item["sha"].as_str().is_some_and(|sha| !sha.is_empty())
                && item["author"]["login"].as_str() == Some("dependabot[bot]")
                && item["commit"]["verification"]["verified"].as_bool() == Some(true)
        })
}

pub fn run() -> ToolResult {
    let event: Value = serde_json::from_slice(&fs::read(std::env::var("GITHUB_EVENT_PATH")?)?)?;
    let repo = event["repository"]["full_name"]
        .as_str()
        .ok_or("missing repository")?;
    let parts: Vec<_> = repo.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
    {
        return Err("invalid repository name".into());
    }
    let pr = event["pull_request"]["number"]
        .as_u64()
        .filter(|number| *number > 0)
        .ok_or("missing pull request number")?;
    let head = event["pull_request"]["head"]["sha"]
        .as_str()
        .ok_or("missing pull request head")?;
    let endpoint = format!("repos/{repo}/pulls/{pr}/commits?per_page=100");
    let output = Command::new("gh")
        .args(["api", &endpoint, "--paginate", "--slurp"])
        .output()?;
    if !output.status.success() {
        return Err("could not fetch Dependabot commit provenance".into());
    }
    let pages: Vec<Vec<Value>> = serde_json::from_slice(&output.stdout)?;
    let commits: Vec<_> = pages.into_iter().flatten().collect();
    if !verified_bot_commits(&commits, head) {
        return Err("the checked batch must contain only verified Dependabot commits and match the event head".into());
    }
    println!("Dependabot provenance verified; required CI gates decide eligibility.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::verified_bot_commits;
    use serde_json::{Value, json};

    fn commit(sha: &str) -> Value {
        json!({"sha": sha, "author": {"login": "dependabot[bot]"},
            "commit": {"verification": {"verified": true}}})
    }

    #[test]
    fn signed_bot_batch_needs_no_version_metadata_or_review() {
        assert!(verified_bot_commits(
            &[commit("first"), commit("head")],
            "head"
        ));
    }

    #[test]
    fn empty_batch_fails_closed() {
        assert!(!verified_bot_commits(&[], "head"));
    }

    #[test]
    fn changed_head_fails_closed() {
        assert!(!verified_bot_commits(&[commit("new")], "head"));
    }

    #[test]
    fn every_commit_must_be_from_dependabot() {
        for author in ["contributor", "github-actions[bot]", "dependabot"] {
            let mut first = commit("first");
            first["author"]["login"] = json!(author);
            assert!(!verified_bot_commits(&[first, commit("head")], "head"));
        }
    }

    #[test]
    fn every_commit_must_have_a_verified_signature() {
        for verified in [json!(false), Value::Null, json!("true")] {
            let mut first = commit("first");
            first["commit"]["verification"]["verified"] = verified;
            assert!(!verified_bot_commits(&[first, commit("head")], "head"));
        }
    }

    #[test]
    fn missing_provenance_fails_closed() {
        for incomplete in [
            json!({}),
            json!({"author": null}),
            json!({"author": {"login": "dependabot[bot]"}}),
        ] {
            assert!(!verified_bot_commits(&[incomplete, commit("head")], "head"));
        }
    }

    #[test]
    fn missing_or_empty_sha_fails_closed() {
        let mut missing = commit("first");
        missing.as_object_mut().unwrap().remove("sha");
        for first in [missing, commit("")] {
            assert!(!verified_bot_commits(&[first, commit("head")], "head"));
        }
        assert!(!verified_bot_commits(&[commit("")], ""));
    }
}
