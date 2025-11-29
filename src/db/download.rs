//! src/download.rs
//! Handles fetching the repository index and downloading package files.

use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

// --- Data Structures for index.json ---
// These structs mirror the structure of our repository index file.

/// Represents a single package entry in the index.
#[derive(Deserialize, Debug, Clone)]
pub struct PackageEntry {
    pub latest_version: String,
    pub download_url: String,
    pub description: String,
}

/// Represents the entire repository index file (index.json).
#[derive(Deserialize, Debug, Clone)]
pub struct RepoIndex {
    pub packages: HashMap<String, PackageEntry>,
}

// --- Public API ---

/// Fetches and parses the repository index from a given base URL.
pub fn fetch_index(repo_url: &str) -> Result<RepoIndex, Box<dyn std::error::Error>> {
    let index_url = format!("{}/index.json", repo_url.trim_end_matches('/'));
    
    let response = reqwest::blocking::get(&index_url)?
        .error_for_status()? // Fail on HTTP errors like 404
        .json::<RepoIndex>()?;
        
    Ok(response)
}

/// Downloads a file from a URL to a destination path, showing a progress bar.
pub async fn download_file_with_progress(
    url: &str,
    dest_path: &Path,
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
    
    // Stream the download chunk by chunk.
    while let Some(chunk) = response.chunk().await? {
        dest_file.write_all(&chunk)?;
        pb.inc(chunk.len() as u64);
    }

    pb.finish_with_message("Download complete");
    Ok(())
}