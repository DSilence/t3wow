//! The updater against a fake GitHub: find the release, download this
//! platform's binary and check it against SHA256SUMS.

use std::path::PathBuf;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use t3wow::update::{self, asset_name};
use tiny_http::{Response, Server};

const BINARY: &[u8] = b"pretend this is a new t3wow binary";

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Serves a release `tag` whose SHA256SUMS lists `listed_hash` for this
/// platform, laid out like github.com: `/releases/latest` redirects to the
/// release's tag and the files redirect once more, to the download host.
fn fake_github(tag: &str, listed_hash: String) -> String {
    let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
    let base = format!("http://{}", server.server_addr().to_ip().unwrap());
    let asset = asset_name();
    let sums = format!("{listed_hash}  {asset}\n0000  t3wow-other-target\n");
    let (tag, redirect_base) = (tag.to_string(), base.clone());
    std::thread::spawn(move || {
        let redirect = |to: String| {
            Response::empty(302)
                .with_header(tiny_http::Header::from_bytes("Location", to.into_bytes()).unwrap())
                .boxed()
        };
        for request in server.incoming_requests() {
            let url = request.url().to_string();
            let response = if url == "/o/r/releases/latest" {
                redirect(format!("{redirect_base}/o/r/releases/tag/{tag}"))
            } else if let Some(name) = url.strip_prefix(&format!("/o/r/releases/download/{tag}/")) {
                redirect(format!("{redirect_base}/assets/{name}"))
            } else if url == "/assets/SHA256SUMS" {
                Response::from_data(sums.clone().into_bytes()).boxed()
            } else if url == format!("/assets/{asset}") {
                Response::from_data(BINARY.to_vec()).boxed()
            } else {
                Response::empty(404).boxed()
            };
            let _ = request.respond(response);
        }
    });
    format!("{base}/o/r/releases/latest")
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("t3wow-update-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn downloads_and_verifies_a_newer_release() {
    let latest = fake_github("v9.0.0", sha256(BINARY));
    let release = update::check(&latest, "0.1.0").unwrap().expect("a newer release");
    assert_eq!(release.version, "9.0.0");
    let dir = temp_dir("ok");
    let file = update::download(&release, &dir).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), BINARY);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ignores_releases_that_are_not_newer() {
    let latest = fake_github("v9.0.0", sha256(BINARY));
    assert_eq!(update::check(&latest, "9.0.0").unwrap(), None);
}

#[test]
fn rejects_a_download_with_the_wrong_checksum() {
    let latest = fake_github("v9.0.0", sha256(b"something else"));
    let release = update::check(&latest, "0.1.0").unwrap().unwrap();
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

#[test]
fn finds_nothing_without_releases() {
    let server = Server::http("127.0.0.1:0").unwrap();
    let latest = format!("http://{}/o/r/releases/latest", server.server_addr().to_ip().unwrap());
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let _ = request.respond(Response::empty(404));
        }
    });
    assert_eq!(update::check(&latest, "0.1.0").unwrap(), None);
}
