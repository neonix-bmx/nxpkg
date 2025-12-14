//! src/download.rs
//! Handles fetching the repository index and downloading package files.

use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use sha2::{Digest, Sha256};
use base64::{engine::general_purpose, Engine as _};

// --- Data Structures for index.json ---
// These structs mirror the structure of our repository index file.

/// Represents a single package entry in the index.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PackageEntry {
    pub latest_version: String,
    pub download_url: String,
    pub description: String,
    #[serde(default)]
    pub sha256: Option<String>,
}

/// Represents the entire repository index file (index.json).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RepoIndex {
    pub packages: HashMap<String, PackageEntry>,
}

// --- Public API ---

/// Fetches and parses the repository index from a given base URL (async).
pub async fn fetch_index(repo_url: &str) -> Result<RepoIndex, Box<dyn std::error::Error>> {
    fetch_index_verified(repo_url, None, false).await
}

/// Fetch index.json and, optionally, verify Ed25519 signature using a base64 public key file.
pub async fn fetch_index_verified(
    repo_url: &str,
    pubkey_path: Option<&Path>,
    require_signature: bool,
) -> Result<RepoIndex, Box<dyn std::error::Error>> {
    let base = repo_url.trim_end_matches('/');
    let index_url = format!("{}/index.json", base);
    let sig_url = format!("{}/index.json.sig", base);
    let client = reqwest::Client::new();

    let index_bytes = client
        .get(&index_url)
        .send()
        .await?
        .error_for_status()? // Fail on HTTP errors like 404
        .bytes()
        .await?;

    if let Some(pubkey_path) = pubkey_path {
        // Try signature verification
        let sig_bytes_b64 = client
            .get(&sig_url)
            .send()
            .await?;
        if sig_bytes_b64.status().is_success() {
            let sig_text = sig_bytes_b64.text().await?;
            let sig_raw = general_purpose::STANDARD
                .decode(sig_text.trim())
                .map_err(|e| format!("invalid base64 in index.json.sig: {}", e))?;
            let pk_b64 = std::fs::read_to_string(pubkey_path)?;
            let pk_raw = general_purpose::STANDARD
                .decode(pk_b64.trim())
                .map_err(|e| format!("invalid base64 in pubkey file {}: {}", pubkey_path.display(), e))?;
            let verified = crate::trust::verify_ed25519_index(&index_bytes, &sig_raw, &pk_raw);
            if !verified {
                if require_signature {
                    return Err("index signature verification failed".into());
                }
            }
        } else if require_signature {
            return Err("index signature not found and signature required".into());
        }
    } else if require_signature {
        return Err("signature required but no pubkey configured".into());
    }

    let idx: RepoIndex = serde_json::from_slice(&index_bytes)?;
    Ok(idx)
}

/// Downloads a file from a URL to a destination path, showing a progress bar.
pub async fn download_file_with_progress(
    url: &str,
    dest_path: &Path,
    expected_sha256: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let mut response = client.get(url).send().await?.error_for_status()?;

    // Get total file size from headers, if available.
    let total_size = response.content_length().unwrap_or(0);

    // Create a progress bar.
    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec})")?
            .progress_chars("#>-"),
    );

    let mut dest_file = File::create(dest_path)?;
    let mut hasher = Sha256::new();
    
    // Stream the download chunk by chunk.
    while let Some(chunk) = response.chunk().await? {
        hasher.update(&chunk);
        dest_file.write_all(&chunk)?;
        pb.inc(chunk.len() as u64);
    }

    // Finalize checksum and verify if provided
    let checksum_hex = hex::encode(hasher.finalize());
    if let Some(expected) = expected_sha256 {
        let expected_norm = expected.trim().to_lowercase();
        if checksum_hex != expected_norm {
            pb.abandon_with_message("Download failed: SHA-256 mismatch");
            let _ = fs::remove_file(dest_path);
            return Err(format!(
                "SHA-256 mismatch: expected {}, got {}",
                expected_norm, checksum_hex
            ).into());
        }
        pb.finish_with_message("Download complete (verified)");
    } else {
        pb.finish_with_message("Download complete");
    }

    Ok(())
}