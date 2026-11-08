use crate::{ToolResult, configuration};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path, time::Duration};

fn immutable_image(image: &str) -> bool {
    let digest = image
        .strip_prefix("sha256:")
        .or_else(|| image.split_once("@sha256:").map(|(_, d)| d));
    digest.is_some_and(|d| d.len() == 64 && d.bytes().all(|c| c.is_ascii_hexdigit()))
        && !image
            .bytes()
            .any(|c| c.is_ascii_whitespace() || matches!(c, b'$' | b'\'' | b'"' | b'`'))
}
fn write_new(path: &Path, contents: &str, private: bool) -> ToolResult {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Secret bind-mounts must be readable by container UID 65534; the
        // parent directory is 0700 and is never mounted as a whole.
        options.mode(if private { 0o600 } else { 0o444 });
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // A restrictive host umask must not break the non-root secret mount.
        let mode = if private { 0o600 } else { 0o444 };
        file.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    Ok(())
}
pub fn initialize(directory: &str, image: &str) -> ToolResult {
    if !immutable_image(image) {
        return Err("an immutable image digest is required".into());
    }
    if directory.is_empty()
        || directory
            .bytes()
            .any(|c| c.is_ascii_whitespace() || matches!(c, b'$' | b'\'' | b'"' | b'`'))
    {
        return Err("directory must be a simple path without whitespace or interpolation".into());
    }
    let path = Path::new(directory);
    // Refuse reuse: no silent password rotation, overwrite or deletion.
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    fs::create_dir(path.join("secrets"))?;
    let database_password = uuid::Uuid::new_v4().simple().to_string();
    let demo_password = uuid::Uuid::new_v4().simple().to_string();
    let db_uri = format!("postgres://authzlab:{database_password}@postgres:5432");
    write_new(
        &path.join("secrets/postgres-password"),
        &database_password,
        false,
    )?;
    write_new(&path.join("secrets/demo-password"), &demo_password, false)?;
    write_new(
        &path.join("secrets/database-url"),
        &format!("{db_uri}/authzlab?sslmode=disable"),
        false,
    )?;
    write_new(
        &path.join("secrets/openfga.yaml"),
        &format!("datastore:\n  engine: postgres\n  uri: {db_uri}/openfga?sslmode=disable\n"),
        false,
    )?;
    let pg = std::env::var("POSTGRES_IMAGE").unwrap_or_else(|_| "postgres:17-bookworm".into());
    let fga = std::env::var("OPENFGA_IMAGE").unwrap_or_else(|_| "openfga/openfga:v1.10.3".into());
    if [pg.as_str(), fga.as_str()]
        .iter()
        .any(|v| v.bytes().any(|c| c.is_ascii_whitespace() || c == b'$'))
    {
        return Err("invalid dependency image".into());
    }
    write_new(
        &path.join("environment.env"),
        &format!(
            "LAB_IMAGE={image}\nLAB_STATE_DIR={directory}\nAPP_PORT=3000\nPOSTGRES_IMAGE={pg}\nOPENFGA_IMAGE={fga}\n"
        ),
        true,
    )?;
    println!(
        "Created {directory}/environment.env and private secret directory; no existing data changed"
    );
    Ok(())
}
fn client() -> ToolResult<Client> {
    Ok(Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()?)
}
fn base() -> String {
    std::env::var("SMOKE_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:3000".into())
}
pub async fn probe() -> ToolResult {
    let response = client()?.get(format!("{}/readyz", base())).send().await?;
    if response.status() != StatusCode::OK {
        return Err("readiness did not return 200".into());
    }
    let body: Value = response.json().await?;
    if body["ready"] != true {
        return Err("readiness is false or malformed".into());
    }
    Ok(())
}
async fn expect(
    client: &Client,
    base: &str,
    method: Method,
    path: &str,
    token: &str,
    expected: StatusCode,
) -> ToolResult {
    let mut request = client
        .request(method, format!("{base}{path}"))
        .bearer_auth(token);
    if path.ends_with("/grants/modify") {
        request = request.json(&json!({"subject":{"type":"user","id":"dave"}}));
    }
    let response = request.send().await?;
    if response.status() != expected {
        return Err(format!("{path}: expected {expected}, got {}", response.status()).into());
    }
    response.bytes().await?;
    Ok(())
}
async fn login(client: &Client, base: &str, user: &str, password: &str) -> ToolResult<String> {
    let data: Value = client
        .post(format!("{base}/session"))
        .json(&json!({"username":user,"password":password}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(data["token"]
        .as_str()
        .ok_or("missing session token")?
        .to_owned())
}
pub async fn acceptance() -> ToolResult {
    if std::env::var("SMOKE_ALLOW_MUTATIONS").as_deref() != Ok("true") {
        return Err("acceptance changes demo permissions; set SMOKE_ALLOW_MUTATIONS=true on an isolated lab".into());
    }
    let client = client()?;
    let base = base();
    let password = configuration::secret("DEMO_PASSWORD")?;
    probe().await?;
    let carol = login(&client, &base, "carol", &password).await?;
    let dave = login(&client, &base, "dave", &password).await?;
    let path = "/projects/acme/warehouses/analytics/tables/requests/grants/modify";
    // Refuse to modify a fixture which already grants dave access.
    expect(
        &client,
        &base,
        Method::GET,
        "/projects/acme/warehouses/analytics/tables/requests",
        &dave,
        StatusCode::FORBIDDEN,
    )
    .await?;
    let exercise = async {
        expect(
            &client,
            &base,
            Method::PUT,
            path,
            &carol,
            StatusCode::NO_CONTENT,
        )
        .await?;
        expect(
            &client,
            &base,
            Method::GET,
            "/projects/acme/warehouses/analytics/tables/requests",
            &dave,
            StatusCode::OK,
        )
        .await?;
        expect(
            &client,
            &base,
            Method::DELETE,
            path,
            &carol,
            StatusCode::NO_CONTENT,
        )
        .await?;
        expect(
            &client,
            &base,
            Method::GET,
            "/projects/acme/warehouses/analytics/tables/requests",
            &dave,
            StatusCode::FORBIDDEN,
        )
        .await
    }
    .await;
    let cleanup = expect(
        &client,
        &base,
        Method::DELETE,
        path,
        &carol,
        StatusCode::NO_CONTENT,
    )
    .await;
    for token in [&carol, &dave] {
        expect(
            &client,
            &base,
            Method::DELETE,
            "/session",
            token,
            StatusCode::NO_CONTENT,
        )
        .await?;
    }
    cleanup?;
    exercise?;
    let version: Value = client
        .get(format!("{base}/version"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    println!(
        "{}",
        json!({"accepted":true,"statuses":[403,204,200,204,403],"version":version})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{immutable_image, initialize};
    #[test]
    fn reject_mutable_or_interpolated_image_names() {
        assert!(!immutable_image("repo:latest"));
        assert!(!immutable_image("sha256:abc"));
        assert!(immutable_image(&format!("repo@sha256:{}", "a".repeat(64))));
    }
    #[test]
    fn environment_initialization_does_not_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("stage");
        let image = format!("sha256:{}", "a".repeat(64));
        initialize(path.to_str().unwrap(), &image).unwrap();
        let before = std::fs::read(path.join("secrets/demo-password")).unwrap();
        assert!(initialize(path.to_str().unwrap(), &image).is_err());
        assert_eq!(
            std::fs::read(path.join("secrets/demo-password")).unwrap(),
            before
        );
    }
}
