//! src/db/upload.rs
//! Uploads .nxpkg files to a repository and updates index.json with checksum info.

use crate::buildins::meta::PackageRecipe;
use crate::db::download::{fetch_index_verified, PackageEntry, RepoIndex, ArchAsset};
use hex;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use sha2::{Digest, Sha256};
use base64::{engine::general_purpose, Engine as _};
use ed25519_dalek::Signer;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

/// Compute SHA-256 checksum of a file, returning lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Upload a local file to an exact destination URL using HTTP PUT.
/// If `bearer_token` is provided, include `Authorization: Bearer <token>` header.
pub async fn upload_file_put(
    destination_url: &str,
    local_path: &Path,
    bearer_token: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    let mut headers = HeaderMap::new();
    if let Some(tok) = bearer_token {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", tok))?,
        );
    }

    let file = File::open(local_path)?;
    let pb = ProgressBar::new(file.metadata()?.len());
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes}")?
            .progress_chars("#>-")
    );

    // For simplicity, read into memory; for huge files, switch to streaming upload
    let body = std::fs::read(local_path)?;
    let resp = client
        .put(destination_url)
        .headers(headers)
        .body(body)
        .send()
        .await?;

    // We can't easily hook progress for PUT with Body-from-file here; we showed a static bar.
    // For real-time progress, switch to a custom stream.
    if !resp.status().is_success() {
        pb.abandon_with_message("Upload failed");
        return Err(format!(
            "Upload failed (HTTP {}): {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        ).into());
    }

    pb.finish_with_message("Upload complete");
    Ok(())
}

/// Publishes a built package: uploads its .nxpkg to repo and updates index.json.
/// - repo_url: base URL of repository (e.g., https://host/releases)
/// - nxpkg_path: local path to the built archive (e.g., /tmp/pkg-1.0.0.nxpkg)
/// - recipe: the recipe used to build (for name/version/architectures)
/// - description: optional description string to appear in index.json
/// - bearer_token: optional Bearer token for auth
pub async fn upload_and_update_index(
    repo_url: &str,
    nxpkg_path: &Path,
    recipe: &PackageRecipe,
    description: Option<&str>,
    bearer_token: Option<&str>,
    // optional signing of the resulting index.json with an ed25519 private key (base64 keypair 64 bytes)
    sign_with_keypair_b64: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let filename = format!("{}-{}.nxpkg", recipe.package.name, recipe.package.version);
    let download_url = format!(
        "{}/{}",
        repo_url.trim_end_matches('/'),
        filename
    );

    // 1) Compute checksum locally
    let checksum = sha256_file(nxpkg_path)?;

    // 2) Upload the .nxpkg
    upload_file_put(&download_url, nxpkg_path, bearer_token).await?;

    // 3) Fetch or init index.json
    let mut index: RepoIndex = match fetch_index_verified(repo_url, None, false).await {
        Ok(idx) => idx,
        Err(_) => RepoIndex { packages: std::collections::HashMap::new() },
    };

    // 4) Update entry with per-architecture asset
    let arch_canonical = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        "arm" => "arm",
        "i686" | "x86" => "i686",
        other => other,
    }.to_string();

    let mut entry = index.packages.remove(&recipe.package.name).unwrap_or(PackageEntry{
        latest_version: recipe.package.version.clone(),
        description: description.unwrap_or("").to_string(),
        download_url: None,
        sha256: None,
        architectures: Some(std::collections::HashMap::new()),
    });

    // Ensure architectures map exists
    if entry.architectures.is_none() { entry.architectures = Some(std::collections::HashMap::new()); }
    let map = entry.architectures.as_mut().unwrap();
    map.insert(arch_canonical.clone(), ArchAsset { download_url: download_url.clone(), sha256: Some(checksum) });

    // Update metadata
    entry.latest_version = recipe.package.version.clone();
    entry.description = description.unwrap_or("").to_string();

    // For backward compatibility, also set legacy fields to this asset
    entry.download_url = Some(download_url.clone());
    entry.sha256 = map.get(&arch_canonical).and_then(|a| a.sha256.clone());

    index.packages.insert(recipe.package.name.clone(), entry);

    // 5) Upload updated index.json via PUT
    let client = reqwest::Client::new();
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Some(tok) = bearer_token {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", tok))?,
        );
    }

    let index_url = format!("{}/index.json", repo_url.trim_end_matches('/'));
    let body = serde_json::to_vec(&index).unwrap();
    let resp = client
        .put(&index_url)
        .headers(headers.clone())
        .body(body.clone())
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(format!(
            "Failed to upload index.json (HTTP {}): {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        ).into());
    }

    // If signing is requested, create index.json.sig and upload it next to index.json
    if let Some(kp_b64) = sign_with_keypair_b64 {
        let keypair_bytes = general_purpose::STANDARD.decode(kp_b64.trim())?;
        if keypair_bytes.len() != 64 { return Err("ed25519 keypair must be 64 bytes (base64)".into()); }
        let secret: ed25519_dalek::SigningKey = ed25519_dalek::SigningKey::from_bytes((&keypair_bytes[0..32]).try_into().unwrap());
        let sig = secret.sign(&body);
        let sig_b64 = general_purpose::STANDARD.encode(sig.to_bytes());

        let sig_url = format!("{}.sig", &index_url);
        let resp_sig = client
            .put(&sig_url)
            .headers(headers)
            .body(sig_b64)
            .send()
            .await?;
        if !resp_sig.status().is_success() {
            return Err(format!(
                "Failed to upload index.json.sig (HTTP {}): {}",
                resp_sig.status(),
                resp_sig.text().await.unwrap_or_default()
            ).into());
        }
    }

    Ok(())
}
