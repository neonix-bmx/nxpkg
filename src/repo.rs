use serde::Deserialize;
use colored::*;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::collections::BTreeMap;
use std::fs;
// src/buildins/mod.rs
// This module handles package creation from source (like AUR).

// Make the `meta` module (for parsing .cfg files) public.

// A standardized struct to hold repository info from any source (GitHub, GitLab, etc.)
#[derive(Debug, Clone)]
pub struct RepoInfo {
    pub name: String,
    pub owner: String,
    pub clone_url: String,
    pub source: String, // "GitHub" or "GitLab"
}

// Structs for deserializing the GitHub API response
#[derive(Deserialize, Debug)]
struct GitHubOwner {
    login: String,
}

#[derive(Deserialize, Debug)]
struct GitHubRepo {
    full_name: String,
    owner: GitHubOwner,
    clone_url: String,
}

#[derive(Deserialize, Debug)]
struct GitHubSearchResult {
    items: Vec<GitHubRepo>,
}

// Structs for deserializing the GitLab API response
#[derive(Deserialize, Debug)]
struct GitLabRepo {
    path_with_namespace: String,
    owner: Option<GitLabOwner>, // Owner info might not always be present
    http_url_to_repo: String,
}

#[derive(Deserialize, Debug)]
struct GitLabOwner {
    name: String,
}


// --- Private Search Functions ---

/// Searches GitHub for repositories.
fn search_github(term: &str) -> Result<Vec<RepoInfo>, Box<dyn std::error::Error>> {
    let url = format!("https://api.github.com/search/repositories?q={}", term);
    let client = reqwest::blocking::Client::new();
    
    let response = client.get(&url)
        .header("User-Agent", "nxpkg-buildins-rust-app") // GitHub API requires a User-Agent
        .send()?
        .json::<GitHubSearchResult>()?;

    let repos = response.items.into_iter().map(|repo| RepoInfo {
        name: repo.full_name,
        owner: repo.owner.login,
        clone_url: repo.clone_url,
        source: "GitHub".to_string(),
    }).collect();

    Ok(repos)
}

/// Searches GitLab for repositories.
fn search_gitlab(term: &str) -> Result<Vec<RepoInfo>, Box<dyn std::error::Error>> {
    let url = format!("https://gitlab.com/api/v4/projects?search={}", term);
    
    let response = reqwest::blocking::get(&url)?
        .json::<Vec<GitLabRepo>>()?;

    let repos = response.into_iter().map(|repo| RepoInfo {
        name: repo.path_with_namespace,
        owner: repo.owner.map_or_else(|| "Unknown".to_string(), |o| o.name),
        clone_url: repo.http_url_to_repo,
        source: "GitLab".to_string(),
    }).collect();

    Ok(repos)
}

// --- Config-based repo list loading ---

fn user_repo_cfg_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_next::home_dir().unwrap_or_else(|| PathBuf::from("~")).join(".config"))
        .join("nxpkg/repos.cfg")
}

fn default_repo_cfg_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    paths.push(PathBuf::from("/etc/nxpkg/repos.cfg"));
    // XDG or ~/.config fallback
    let user_cfg_base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs_next::home_dir().map(|h| h.join(".config")));
    if let Some(base) = user_cfg_base {
        paths.push(base.join("nxpkg/repos.cfg"));
    }
    paths
}

fn parse_repo_cfg(content: &str) -> Vec<RepoInfo> {
    let mut in_repos = false;
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') { continue; }
        if line.starts_with('[') && line.ends_with(']') {
            let sec = &line[1..line.len()-1];
            in_repos = sec.eq_ignore_ascii_case("repos");
            continue;
        }
        if !in_repos { continue; }
        if let Some((name, url)) = line.split_once('=') {
            let name = name.trim().to_string();
            let url = url.trim().to_string();
            // Heuristic parse to fill owner/source
            let lower = url.to_lowercase();
            let source = if lower.contains("github.com") { "GitHub" } else if lower.contains("gitlab.com") { "GitLab" } else { "Custom" };
            // Extract owner from path
            let owner = if let Some(idx) = url.find("github.com/") {
                url[idx+"github.com/".len()..].split('/').next().unwrap_or("").to_string()
            } else if let Some(idx) = url.find("gitlab.com/") {
                url[idx+"gitlab.com/".len()..].split('/').next().unwrap_or("").to_string()
            } else {
                String::new()
            };
            // Normalize display name as owner/repo if possible
            let display_name = if !owner.is_empty() {
                let rest = url.split('/').rev().next().unwrap_or("");
                let repo = rest.trim_end_matches(".git");
                format!("{}/{}", owner, repo)
            } else {
                name.clone()
            };
            out.push(RepoInfo { name: display_name, owner, clone_url: url, source: source.to_string() });
        }
    }
    out
}

pub fn configured_repos() -> Vec<RepoInfo> {
    let mut repos = Vec::new();
    for p in default_repo_cfg_paths() {
        if p.exists() {
            match fs::read_to_string(&p) {
                Ok(s) => repos.extend(parse_repo_cfg(&s)),
                Err(e) => eprintln!("{} failed reading {}: {}", "Warning:".yellow(), p.display(), e),
            }
        }
    }
    repos
}

pub fn search_config_repos(term: &str) -> Vec<RepoInfo> {
    let t = term.to_lowercase();
    configured_repos()
        .into_iter()
        .filter(|r| r.name.to_lowercase().contains(&t) || r.clone_url.to_lowercase().contains(&t))
        .collect()
}

// --- Config management helpers ---

pub fn add_repo_entry(name: &str, url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    let user_path = user_repo_cfg_path();
    if let Ok(content) = fs::read_to_string(&user_path) {
        let mut in_repos = false;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') { continue; }
            if line.starts_with('[') && line.ends_with(']') {
                in_repos = &line[1..line.len()-1] == "repos";
                continue;
            }
            if !in_repos { continue; }
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    map.insert(name.trim().to_string(), url.trim().to_string());

    if let Some(parent) = user_path.parent() { let _ = fs::create_dir_all(parent); }
    let mut out = String::new();
    out.push_str("[repos]\n");
    for (k, v) in map { out.push_str(&format!("{} = {}\n", k, v)); }
    fs::write(&user_path, out)?;
    Ok(())
}

pub fn remove_repo_entry(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let user_path = user_repo_cfg_path();
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(content) = fs::read_to_string(&user_path) {
        let mut in_repos = false;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') { continue; }
            if line.starts_with('[') && line.ends_with(']') {
                in_repos = &line[1..line.len()-1] == "repos";
                continue;
            }
            if !in_repos { continue; }
            if let Some((k, v)) = line.split_once('=') {
                let key = k.trim().to_string();
                if key != name { map.insert(key, v.trim().to_string()); }
            }
        }
    }
    if let Some(parent) = user_path.parent() { let _ = fs::create_dir_all(parent); }
    let mut out = String::new();
    out.push_str("[repos]\n");
    for (k, v) in map { out.push_str(&format!("{} = {}\n", k, v)); }
    fs::write(&user_path, out)?;
    Ok(())
}

pub fn select_repo_from_config(term: Option<&str>) -> Result<RepoInfo, Box<dyn std::error::Error>> {
    let mut list = configured_repos();
    if let Some(t) = term {
        let tl = t.to_lowercase();
        list.retain(|r| r.name.to_lowercase().contains(&tl) || r.clone_url.to_lowercase().contains(&tl));
    }
    if list.is_empty() { return Err("No configured repositories matched.".into()); }
    if list.len() == 1 { return Ok(list.remove(0)); }

    println!("\n{}", "Multiple configured repositories found. Please choose one:".green());
    for (i, repo) in list.iter().enumerate() {
        println!(
            "  [{}] {} ({}) - by {}",
            (i + 1).to_string().bold(),
            repo.name.cyan(),
            repo.source.yellow(),
            repo.owner
        );
    }
    loop {
        print!("{}", "\nEnter your choice (number): ".bold());
        io::stdout().flush()?;
        let mut choice = String::new();
        io::stdin().read_line(&mut choice)?;
        match choice.trim().parse::<usize>() {
            Ok(n) if n > 0 && n <= list.len() => return Ok(list.remove(n - 1)),
            _ => eprintln!("{}", "Invalid input. Please enter a number from the list.".red()),
        }
    }
}

// --- Public API ---

/// Finds a repository by searching GitHub and GitLab, then prompts the user to select one.
pub fn find_and_select_repo(term: &str) -> Result<RepoInfo, Box<dyn std::error::Error>> {
    // Prefer configured repos first
    let mut all_repos = search_config_repos(term);
    if !all_repos.is_empty() {
        println!("{}", "Found matches in configured repos".cyan());
    } else {
        // Fallback to remote searches
        println!("{}", "Searching on GitHub...".cyan());
        match search_github(term) {
            Ok(repos) => all_repos.extend(repos),
            Err(e) => eprintln!("{} {}", "GitHub search failed:".yellow(), e),
        }

        println!("{}", "Searching on GitLab...".cyan());
        match search_gitlab(term) {
            Ok(repos) => all_repos.extend(repos),
            Err(e) => eprintln!("{} {}", "GitLab search failed:".yellow(), e),
        }
    }

    // --- Process Results ---

    if all_repos.is_empty() {
        return Err("No repositories found.".into());
    }

    if all_repos.len() == 1 {
        println!("{}", "Found exactly one match. Proceeding automatically.".green());
        return Ok(all_repos.remove(0));
    }

    // --- Prompt User for Selection ---
    
    println!("\n{}", "Multiple repositories found. Please choose one:".green());
    
    // Display up to 10 options
    let display_count = all_repos.len().min(10);
    for (i, repo) in all_repos.iter().enumerate().take(display_count) {
        println!(
            "  [{}] {} ({}) - by {}",
            (i + 1).to_string().bold(),
            repo.name.cyan(),
            repo.source.yellow(),
            repo.owner
        );
    }

    if all_repos.len() > 10 {
        println!("  [{}] {}", "11".bold(), "Show all contributors/options... (Not implemented yet)".dimmed());
    }

    loop {
        print!("{}", "\nEnter your choice (number): ".bold());
        io::stdout().flush()?; // Ensure the prompt is shown before reading input

        let mut choice = String::new();
        io::stdin().read_line(&mut choice)?;

        match choice.trim().parse::<usize>() {
            Ok(n) if n > 0 && n <= display_count => {
                return Ok(all_repos.remove(n - 1));
            }
            _ => {
                eprintln!("{}", "Invalid input. Please enter a number from the list.".red());
            }
        }
    }
}
