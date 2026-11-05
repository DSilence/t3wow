//! The updater against a fake GitHub: find the release, download this
//! platform's binary and check it against SHA256SUMS.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use sha2::{Digest, Sha256};
use t3wow::update::{self, asset_name};
use tiny_http::{Response, Server};

const BINARY: &[u8] = b"pretend this is a new t3wow binary";

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Serves a release `tag` whose SHA256SUMS lists `listed_hash` for this platform.
fn fake_github(tag: &str, listed_hash: String) -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
    let base = format!("http://{}", server.server_addr().to_ip().unwrap());
    let asset = asset_name();
    let release = json!({
        "tag_name": tag,
        "draft": false,
        "prerelease": false,
        "assets": [
            { "name": asset, "browser_download_url": format!("{base}/download/{asset}") },
            { "name": "SHA256SUMS", "browser_download_url": format!("{base}/download/SHA256SUMS") },
        ],
    })
    .to_string();
    let sums = format!("{listed_hash}  {asset}\n0000  t3wow-other-target\n");
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let body: Vec<u8> = match request.url() {
                "/releases/latest" => release.clone().into_bytes(),
                "/download/SHA256SUMS" => sums.clone().into_bytes(),
                url if url == format!("/download/{asset}") => BINARY.to_vec(),
                _ => {
                    let _ = request.respond(Response::empty(404));
                    continue;
                }
            };
            let _ = request.respond(Response::from_data(body));
        }
    });
    format!("{base}/releases/latest")
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("t3wow-update-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn downloads_and_verifies_a_newer_release() {
    let api = fake_github("v9.0.0", sha256(BINARY));
    let release = update::check(&api, "0.1.0").unwrap().expect("a newer release");
    assert_eq!(release.version, "9.0.0");
    let dir = temp_dir("ok");
    let file = update::download(&release, &dir).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), BINARY);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ignores_releases_that_are_not_newer() {
    let api = fake_github("v9.0.0", sha256(BINARY));
    assert_eq!(update::check(&api, "9.0.0").unwrap(), None);
}

#[test]
fn rejects_a_download_with_the_wrong_checksum() {
    let api = fake_github("v9.0.0", sha256(b"something else"));
    let release = update::check(&api, "0.1.0").unwrap().unwrap();
    let dir = temp_dir("bad");
    let error = update::download(&release, &dir).unwrap_err();
    assert!(error.to_string().contains("checksum"), "{error}");
    assert!(!dir.join(asset_name()).exists());
}

#[test]
fn replaces_a_file_in_place() {
    let dir = temp_dir("replace");
    std::fs::create_dir_all(&dir).unwrap();
    let (target, source) = (dir.join("t3wow"), dir.join("new"));
    std::fs::write(&target, "old").unwrap();
    std::fs::write(&source, "new").unwrap();
    update::replace_file(&target, &source).unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    let _ = std::fs::remove_dir_all(dir);
}
