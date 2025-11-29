use serde::Deserialize;
use colored::*;
use std::io::{self, Write};
// src/buildins/mod.rs
// This module handles package creation from source (like AUR).

// Make the `meta` module (for parsing .cfg files) public.
pub mod meta;


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

// --- Public API ---

/// Finds a repository by searching GitHub and GitLab, then prompts the user to select one.
pub fn find_and_select_repo(term: &str) -> Result<RepoInfo, Box<dyn std::error::Error>> {
    // Perform searches (can be parallelized in the future)
    let mut all_repos = vec![];
    
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
