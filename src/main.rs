mod db;
mod compress;
mod buildins;
mod repo;
mod config;
mod trust;
use crate::db::download;
use crate::db::upload;
use crate::buildins::buildpkg;
use crate::buildins::chroot::ChrootEnv;
use crate::buildins::meta::{BuildInfo, InstallInfo, PackageInfo, PackageRecipe};
use crate::buildins::profile::BuildProfile;
use crate::config::AppConfig;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};


pub use compress::decompress_tarball;
pub use db::PackageManagerDB;
use clap::{Parser, Subcommand, ValueEnum};
use rusqlite::Connection;
use indicatif::{ProgressBar, ProgressStyle};
use colored::*;
// Indicates version of the nxpkg source code for every ".rs" file
pub const VERSION: &str = "v0.1.0";

/// info
#[derive(Parser)]
#[command(name = "nxpkg")]
#[command(about = "NeoniX PacKaGe Manager for Neonix v1.0")]

struct Cli {
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    /// Installs Package
    Install {
        /// Package name
        name: Option<String>,

        /// Install files locally
        #[arg(short = 'L', long = "local")]
        local: Option<String>,
    },
    /// Removes Packgage
    Remove {
        /// Package name
        name: String,
    },
    Purge {
        /// Package name
        name: String,
    },
    /// Searches for packages in the remote repository
    Search {
        /// The search term
        term: String,
    },
    Debug1 {
        /// Package name
        name: String,
    },
    // Show about of the nxpkg
    About,
    Buildins {
        /// Repository search term or name
        name: String,
        /// Package name (auto-detected for common cases)
        #[arg(short = 'p', long = "package")]
        package: Option<String>,
        /// Package version (auto-detected if possible)
        #[arg(long = "version")]
        version: Option<String>,
        /// Output directory for the .nxpkg artifact
        #[arg(long = "output-dir")]
        output_dir: Option<String>,
        /// Staging directory inside chroot for install (default: /pkg)
        #[arg(long = "staging-dir")]
        staging_dir: Option<String>,
        /// Override build system (cargo|meson|cmake|scons|make)
        #[arg(long = "build-system", value_enum)]
        build_system: Option<BuildSystemKind>,
        /// Extra args for configure/setup step (repeatable)
        #[arg(long = "configure-arg")]
        configure_args: Vec<String>,
        /// Extra args for build step (repeatable)
        #[arg(long = "build-arg")]
        build_args: Vec<String>,
        /// Extra args for install step (repeatable)
        #[arg(long = "install-arg")]
        install_args: Vec<String>,
        /// Save resolved build profile to DB
        #[arg(long = "save-profile")]
        save_profile: bool,
        /// Ignore any stored build profile for this package
        #[arg(long = "no-profile")]
        no_profile: bool,
    },

    /// Manage and select source repositories (from repos.cfg)
    Repos {
        #[command(subcommand)]
        action: RepoAction,
    },

    /// Manage binary repository remotes (for package index/download)
    RepoRemote {
        #[command(subcommand)]
        action: RepoRemoteAction,
    },

    // Show version of the nxpkg
    Version,

    /// Health check (periodic diagnostics)
    Health {
        /// Skip network (don't fetch repository index)
        #[arg(long = "no-network")]
        no_network: bool,
        /// Check chroot prerequisites (check required tools in PATH)
        #[arg(long = "check-chroot")]
        check_chroot: bool,
    },

    /// Publish a built .nxpkg to the repository and update index.json (optionally sign)
    Publish {
        /// Path to .nxpkg file
        file: String,
        /// Optional description to add/update in index.json
        #[arg(short = 'd', long = "desc")]
        desc: Option<String>,
        /// Override repo URL (defaults to config file)
        #[arg(long = "repo")]
        repo: Option<String>,
        /// Bearer token for upload (or set env NXPKG_TOKEN)
        #[arg(long = "token")]
        token: Option<String>,
        /// Base64 ed25519 keypair (64 bytes) for signing index.json (or env NXPKG_SIGN_KEYPAIR_B64)
        #[arg(long = "sign-keypair-b64")]
        sign_keypair_b64: Option<String>,
        /// Read base64 ed25519 keypair from file path
        #[arg(long = "sign-keypair-file")]
        sign_keypair_file: Option<String>,
    },
}

// Subcommands for repo management
#[derive(Subcommand)]
enum RepoAction {
    /// List configured repositories from repos.cfg
    List,
    /// Add or update an entry in user repos.cfg (~/.config/nxpkg/repos.cfg)
    Add { name: String, url: String },
    /// Remove an entry from user repos.cfg
    Remove { name: String },
    /// Choose a repo from configured repos (optionally filter by term)
    Choose { term: Option<String>, #[arg(long = "build")] build: bool, #[arg(long = "print-url")] print_url: bool },
}

// Binary repo remote management
#[derive(Subcommand)]
enum RepoRemoteAction {
    /// List configured binary repo remotes and show active
    List,
    /// Add or update a binary repo remote in user file
    Add { name: String, url: String },
    /// Remove a binary repo remote from user file
    Remove { name: String },
    /// Choose active binary repo remote by name
    Choose { name: String },
    /// Show current effective repo URL
    Current,
}

// Helper enum and function for build system detection
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BuildSystemKind {
    Cargo,
    Meson,
    Cmake,
    Scons,
    Make,
}

impl BuildSystemKind {
    fn priority(self) -> u8 {
        match self {
            BuildSystemKind::Cargo => 0,
            BuildSystemKind::Meson => 1,
            BuildSystemKind::Cmake => 2,
            BuildSystemKind::Scons => 3,
            BuildSystemKind::Make => 4,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            BuildSystemKind::Cargo => "cargo",
            BuildSystemKind::Meson => "meson",
            BuildSystemKind::Cmake => "cmake",
            BuildSystemKind::Scons => "scons",
            BuildSystemKind::Make => "make",
        }
    }
}

#[derive(Debug, Clone)]
struct BuildSystemMatch {
    kind: BuildSystemKind,
    path: PathBuf,
    depth: usize,
}

fn parse_build_system(s: &str) -> Option<BuildSystemKind> {
    match s.trim().to_lowercase().as_str() {
        "cargo" => Some(BuildSystemKind::Cargo),
        "meson" => Some(BuildSystemKind::Meson),
        "cmake" => Some(BuildSystemKind::Cmake),
        "scons" => Some(BuildSystemKind::Scons),
        "make" => Some(BuildSystemKind::Make),
        _ => None,
    }
}

/// Recursively finds the best build system in the cloned repository.
fn find_build_systems(root_path: &Path) -> Vec<BuildSystemMatch> {
    let mut candidates: Vec<BuildSystemMatch> = Vec::new();

    for entry in WalkDir::new(root_path).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = match path.file_name().and_then(|s| s.to_str()) {
            Some(name) => name,
            None => continue,
        };

        let kind = match file_name {
            "Cargo.toml" => Some(BuildSystemKind::Cargo),
            "meson.build" => Some(BuildSystemKind::Meson),
            "CMakeLists.txt" => Some(BuildSystemKind::Cmake),
            "SConstruct" | "SConscript" => Some(BuildSystemKind::Scons),
            "Makefile" | "makefile" | "GNUmakefile" => Some(BuildSystemKind::Make),
            _ => None,
        };

        if let Some(kind) = kind {
            if let Some(parent) = path.parent() {
                let depth = parent.strip_prefix(root_path)
                    .map(|p| p.components().count())
                    .unwrap_or(usize::MAX);
                candidates.push(BuildSystemMatch {
                    kind,
                    path: parent.to_path_buf(),
                    depth,
                });
            }
        }
    }

    candidates
}

fn pick_build_system(
    candidates: &[BuildSystemMatch],
    preferred: Option<BuildSystemKind>,
) -> Option<BuildSystemMatch> {
    if let Some(kind) = preferred {
        let mut matches: Vec<BuildSystemMatch> = candidates
            .iter()
            .filter(|c| c.kind == kind)
            .cloned()
            .collect();
        matches.sort_by_key(|c| c.depth);
        return matches.into_iter().next();
    }

    let mut matches = candidates.to_vec();
    matches.sort_by_key(|c| (c.kind.priority(), c.depth));
    matches.into_iter().next()
}

fn arch_alias() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        "arm" => "arm",
        "x86" | "i686" => "i686",
        "powerpc64" => "ppc64",
        "powerpc64le" => "ppc64le",
        other => other,
    }
}

fn auto_package_name(repo_name_only: &str) -> Option<String> {
    let lower = repo_name_only.to_lowercase();
    let arch = arch_alias();
    if lower.contains("mesa") {
        Some(format!("mesa-{}", arch))
    } else if lower == "linux" || lower.contains("kernel") {
        Some(format!("linux-{}", arch))
    } else {
        None
    }
}

fn prompt_for_package_name() -> io::Result<String> {
    print!("Enter package name: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "package name cannot be empty"));
    }
    Ok(trimmed.to_string())
}

fn run_chroot_command(
    chroot_env: &ChrootEnv,
    command: &str,
    args: &[String],
    cwd: Option<&Path>,
) -> io::Result<std::process::ExitStatus> {
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    chroot_env.run_command(command, &args_ref, cwd)
}

fn resolve_staging_dir(input: Option<String>) -> Result<PathBuf, String> {
    let dir = input.unwrap_or_else(|| "/pkg".to_string());
    let path = PathBuf::from(&dir);
    if !path.is_absolute() {
        return Err(format!("staging dir must be absolute: {}", dir));
    }
    Ok(path)
}

fn resolve_output_dir(input: Option<String>) -> Result<PathBuf, String> {
    let path = match input {
        Some(p) => PathBuf::from(p),
        None => std::env::current_dir().map_err(|e| format!("failed to get current dir: {}", e))?,
    };
    if let Err(e) = std::fs::create_dir_all(&path) {
        return Err(format!("failed to create output dir {}: {}", path.display(), e));
    }
    Ok(path)
}

fn read_cargo_version(repo_path: &Path) -> Option<String> {
    let path = repo_path.join("Cargo.toml");
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    let value: toml::Value = toml::from_str(&content).ok()?;
    value.get("package")
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn git_describe(repo_path: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("describe")
        .arg("--tags")
        .arg("--always")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let s = raw.trim();
    if s.is_empty() { None } else { Some(s.to_string()) }
}

fn resolve_package_version(
    explicit: Option<String>,
    repo_path: &Path,
) -> String {
    if let Some(v) = explicit {
        return v;
    }
    if let Some(v) = read_cargo_version(repo_path) {
        return v;
    }
    if let Some(v) = git_describe(repo_path) {
        return v;
    }
    "0.0.0".to_string()
}

fn build_recipe(
    package_name: &str,
    version: &str,
    build_kind: BuildSystemKind,
    profile: &BuildProfile,
) -> PackageRecipe {
    let mut build_commands = Vec::new();
    build_commands.push(format!("build_system={}", build_kind.as_str()));
    if !profile.configure_args.is_empty() {
        build_commands.push(format!("configure_args={}", profile.configure_args.join(" ")));
    }
    if !profile.build_args.is_empty() {
        build_commands.push(format!("build_args={}", profile.build_args.join(" ")));
    }

    PackageRecipe {
        package: PackageInfo {
            name: package_name.to_string(),
            version: version.to_string(),
            architectures: vec![arch_alias().to_string()],
        },
        build: BuildInfo {
            dependencies: Vec::new(),
            commands: build_commands,
        },
        install: InstallInfo {
            install_params: profile.install_args.clone(),
            installed_files: Vec::new(),
        },
    }
}

// REPO_URL artık /etc veya kullanıcı konfigürasyonundan okunuyor (config::AppConfig)

#[tokio::main]
async fn main() {
    let cfg = AppConfig::load();
    let _ = fs::create_dir_all(cfg.cache_dir.clone());
    if let Some(parent) = cfg.db_path.parent() { let _ = fs::create_dir_all(parent); }

    let cli = Cli::parse();
    let Some(_val) = Connection::open(&cfg.db_path).ok() else { return };
    let db1 = match PackageManagerDB::new(cfg.db_path.to_str().unwrap_or("nxpkg_meta.db")) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("E02: Startup of database is failed: {}", e);
            return;
        }
    };

    match cli.command {
        Commands::Install { name, local } => {
            let pb = ProgressBar::new_spinner();
            pb.enable_steady_tick(std::time::Duration::from_millis(120));
            pb.set_style(ProgressStyle::with_template("{spinner:.blue} {elapsed_precise} {msg}").unwrap());

            let nxpkg_path: PathBuf;
            let package_name_from_source: String;

            if let Some(local_path_str) = local {
                nxpkg_path = PathBuf::from(&local_path_str);
                package_name_from_source = nxpkg_path.file_stem().unwrap_or_default().to_str().unwrap_or_default().to_string();
                pb.set_message(format!("Installing from local package '{}'...", nxpkg_path.display()));
            
            } else if let Some(remote_name) = name {
                pb.set_message(format!("Fetching repository index..."));
                
                let index = match download::fetch_index_verified(&cfg.repo_url, Some(&cfg.pubkey_path), cfg.require_signed_index).await {
                    Ok(i) => i,
                    Err(e) => {
                        pb.finish_with_message(format!("Failed to fetch repository index: {}", e).red().to_string());
                        return;
                    }
                };

                let package_entry = match index.packages.get(&remote_name) {
                    Some(entry) => entry,
                    None => {
                        pb.finish_with_message(format!("Package '{}' not found in the repository.", remote_name).red().to_string());
                        return;
                    }
                };

                // Resolve proper asset for current architecture
                let (asset_url, asset_sha) = match download::resolve_asset_for_current_arch(package_entry) {
                    Some(x) => x,
                    None => {
                        pb.finish_with_message(format!("No compatible asset for '{}' on arch {}.", remote_name, std::env::consts::ARCH).red().to_string());
                        return;
                    }
                };
                
                package_name_from_source = remote_name;
                nxpkg_path = cfg.cache_dir.join(format!("{}.nxpkg", package_name_from_source));

                pb.finish_and_clear();
                
                if let Err(e) = download::download_file_with_progress(&asset_url, &nxpkg_path, asset_sha.as_deref()).await {
                    eprintln!("{}", format!("\nDownload failed: {}", e).red());
                    return;
                }
                
                pb.reset();
                pb.set_message("Download complete. Continuing installation...");

            } else {
                eprintln!("{}", "Error: Must specify a package name or a local file with -L.".red());
                return;
            }

            if let Ok(Some(installed_recipe)) = db1.get_package_metadata(&package_name_from_source) {
                pb.finish_with_message(format!("'{}' v{} is already installed.", installed_recipe.package.name, installed_recipe.package.version).yellow().to_string());
                return;
            }

            pb.set_message(format!("Extracting package '{}'...", package_name_from_source));
            let (mut recipe, installed_files) = match compress::extract_nxpkg(&nxpkg_path) {
                Ok(r) => r,
                Err(e) => {
                    pb.finish_with_message(format!("Failed to install package: {}", e).red().to_string());
                    return;
                }
            };

            // Persist installed file paths into the recipe so uninstall can remove them later
            recipe.install.installed_files = installed_files
                .into_iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            
            pb.set_message("Registering package in database...");
            if let Err(e) = db1.save_package_metadata(&recipe) {
                pb.finish_with_message(format!("Database registration failed: {}", e).red().to_string());
                return;
            }
            
            pb.finish_with_message(format!("Successfully installed '{}' v{}.", recipe.package.name, recipe.package.version).green().to_string());
        }
        Commands::Remove { name } | Commands::Purge { name } => {
            let pb = ProgressBar::new_spinner();
            pb.enable_steady_tick(std::time::Duration::from_millis(120));
            pb.set_style(ProgressStyle::with_template("{spinner:.blue} {msg}").unwrap());
            pb.set_message(format!("Removing {}...", name));
            if let Ok(Some(_)) = db1.get_package_metadata(&name) {
                let _ = db1.rem_package_metadata(&name);
                pb.finish_with_message(format!("{} package is purged.", name).green().to_string());
            } else {
                pb.finish_with_message(format!("{} package is not found.", name).red().to_string());
            }
        }
        Commands::Search { term } => {
            let pb = ProgressBar::new_spinner();
            pb.enable_steady_tick(std::time::Duration::from_millis(120));
            pb.set_style(ProgressStyle::with_template("{spinner:.blue} {elapsed_precise} {msg}").unwrap());
            pb.set_message("Fetching repository index...");

                            let index = match download::fetch_index_verified(&cfg.repo_url, Some(&cfg.pubkey_path), cfg.require_signed_index).await {

                Ok(i) => i,
                Err(e) => {
                    pb.finish_with_message(format!("Failed to fetch repository index: {}", e).red().to_string());
                    return;
                }
            };
            pb.finish_and_clear();

            let term = term.to_lowercase();
            let results: Vec<_> = index.packages.iter()
                .filter(|(name, entry)| 
                    name.to_lowercase().contains(&term) || entry.description.to_lowercase().contains(&term)
                )
                .collect();

            if results.is_empty() {
                println!("{}", "No packages found matching your search term.".yellow());
            } else {
                println!("Found {} package(s):", results.len());
                for (name, entry) in results {
                    println!(
                        "  {} {} - {}",
                        name.bold().cyan(),
                        entry.latest_version.dimmed(),
                        entry.description
                    );
                }
            }
        }
        Commands::Buildins {
            name,
            package,
            version,
            output_dir,
            staging_dir,
            build_system,
            configure_args,
            build_args,
            install_args,
            save_profile,
            no_profile,
        } => {
            let selected_repo = match repo::find_and_select_repo(&name) {
                Ok(repo) => repo,
                Err(e) => {
                    eprintln!("{}", format!("\nBuild process failed: {}", e).red());
                    return;
                }
            };

            use std::process::Command;

            let repo_name_only = selected_repo.name.split('/').last().unwrap_or(&selected_repo.name);
            let package_name = match package {
                Some(name) => name,
                None => match auto_package_name(repo_name_only) {
                    Some(auto_name) => auto_name,
                    None => match prompt_for_package_name() {
                        Ok(name) => name,
                        Err(e) => {
                            eprintln!("{} {}", "Failed to read package name:".red(), e);
                            return;
                        }
                    },
                },
            };
            let staging_dir_in_chroot = match resolve_staging_dir(staging_dir) {
                Ok(dir) => dir,
                Err(e) => {
                    eprintln!("{} {}", "Invalid staging dir:".red(), e);
                    return;
                }
            };
            let output_dir = match resolve_output_dir(output_dir) {
                Ok(dir) => dir,
                Err(e) => {
                    eprintln!("{} {}", "Invalid output dir:".red(), e);
                    return;
                }
            };

            println!(
                "\nProceeding to build '{}' as package '{}'.",
                selected_repo.name.cyan(),
                package_name.cyan()
            );

            let mut profile = if no_profile {
                BuildProfile::new(&package_name)
            } else {
                match db1.get_build_profile(&package_name) {
                    Ok(Some(p)) => p,
                    Ok(None) => BuildProfile::new(&package_name),
                    Err(e) => {
                        eprintln!("{} {}", "Warning: failed to load build profile:".yellow(), e);
                        BuildProfile::new(&package_name)
                    }
                }
            };
            profile.name = package_name.clone();
            if let Some(kind) = build_system {
                profile.build_system = Some(kind.as_str().to_string());
            }
            if !configure_args.is_empty() {
                profile.configure_args = configure_args;
            }
            if !build_args.is_empty() {
                profile.build_args = build_args;
            }
            if !install_args.is_empty() {
                profile.install_args = install_args;
            }

            let pb_clone = ProgressBar::new_spinner();
            pb_clone.enable_steady_tick(std::time::Duration::from_millis(120));
            pb_clone.set_style(ProgressStyle::with_template("{spinner:.green} {elapsed_precise} {msg}").unwrap());

            let clone_path = format!("/tmp/{}", repo_name_only);

            let _ = std::fs::remove_dir_all(&clone_path);

            pb_clone.set_message(format!("Cloning from {}...", selected_repo.clone_url));

            let clone_status = pb_clone.suspend(|| {
                Command::new("git")
                    .arg("clone")
                    .arg(&selected_repo.clone_url)
                    .arg(&clone_path)
                    .status()
            });

            if !clone_status.map_or(false, |s| s.success()) {
                pb_clone.finish_with_message(format!("Failed to clone {}.", selected_repo.name).red().to_string());
                return;
            }
            pb_clone.finish_with_message(format!("Successfully cloned {}.", selected_repo.name).green().to_string());

            let clone_path_obj = std::path::Path::new(&clone_path);
            if clone_path_obj.join(".gitmodules").exists() {
                let pb_submodule = ProgressBar::new_spinner();
                pb_submodule.enable_steady_tick(std::time::Duration::from_millis(120));
                pb_submodule.set_style(ProgressStyle::with_template("{spinner:.cyan} {elapsed_precise} {msg}").unwrap());
                pb_submodule.set_message("Initializing and updating submodules...");

                let submodule_status = pb_submodule.suspend(|| {
                    Command::new("git")
                        .arg("submodule")
                        .arg("update")
                        .arg("--init")
                        .arg("--recursive")
                        .current_dir(&clone_path)
                        .status()
                });

                if !submodule_status.map_or(false, |s| s.success()) {
                    pb_submodule.finish_with_message("Failed to update submodules.".red().to_string());
                    return;
                }
                pb_submodule.finish_with_message("Submodules updated successfully.".green().to_string());
            }

            let pb_build = ProgressBar::new_spinner();
            pb_build.enable_steady_tick(std::time::Duration::from_millis(120));
            pb_build.set_style(ProgressStyle::with_template("{spinner:.yellow} {elapsed_precise} {msg}").unwrap());

            // --- Chroot Setup ---
            let chroot_path = Path::new("/tmp/nxpkg-chroot");
            let chroot_env = ChrootEnv::new(&chroot_path);

            if let Err(e) = chroot_env.prepare() {
                pb_build.finish_with_message(format!("Failed to prepare chroot environment: {}", e).red().to_string());
                let _ = chroot_env.cleanup(); // Attempt to clean up even on failure
                return;
            }

            // Move cloned repo into the chroot build directory
            let chroot_build_dir = chroot_path.join("build");
            std::fs::create_dir_all(&chroot_build_dir).unwrap();
            let staging_host_path = chroot_path.join(
                staging_dir_in_chroot.strip_prefix("/").unwrap_or(&staging_dir_in_chroot)
            );
            let _ = std::fs::remove_dir_all(&staging_host_path);
            if let Err(e) = std::fs::create_dir_all(&staging_host_path) {
                pb_build.finish_with_message(format!("Failed to create staging dir: {}", e).red().to_string());
                let _ = chroot_env.cleanup();
                return;
            }
            let new_repo_path = chroot_build_dir.join(repo_name_only);
            if let Err(e) = std::fs::rename(&clone_path, &new_repo_path) {
                pb_build.finish_with_message(format!("Failed to move repo into chroot: {}", e).red().to_string());
                let _ = chroot_env.cleanup();
                return;
            }

            pb_build.set_message(format!("Detecting build system for {} inside chroot...", selected_repo.name));

            let candidates = find_build_systems(&new_repo_path);
            let preferred_kind = build_system
                .or_else(|| profile.build_system.as_deref().and_then(parse_build_system));
            if preferred_kind.is_none() {
                if let Some(ref bs) = profile.build_system {
                    eprintln!("{} {}", "Warning: unknown build system in profile:".yellow(), bs);
                    profile.build_system = None;
                }
            }
            let mut selected_build = pick_build_system(&candidates, preferred_kind);
            if selected_build.is_none() {
                if let Some(kind) = preferred_kind {
                    selected_build = Some(BuildSystemMatch {
                        kind,
                        path: new_repo_path.clone(),
                        depth: 0,
                    });
                }
            }

            let Some(selected_build) = selected_build else {
                pb_build.finish_with_message(format!("Could not detect a known build system in {}.", selected_repo.name).red().to_string());
                let _ = chroot_env.cleanup();
                return;
            };
            let package_version = resolve_package_version(version, &selected_build.path);

            if save_profile {
                if profile.build_system.is_none() {
                    profile.build_system = Some(selected_build.kind.as_str().to_string());
                }
                if let Err(e) = db1.save_build_profile(&profile) {
                    eprintln!("{} {}", "Failed to save build profile:".red(), e);
                } else {
                    println!("Saved build profile for '{}'.", package_name.cyan());
                }
            }

            let build_path_in_chroot = Path::new("/build").join(repo_name_only);
            let rel = selected_build.path.strip_prefix(&new_repo_path).unwrap_or(Path::new(""));
            let src_dir_chroot = if rel.as_os_str().is_empty() {
                build_path_in_chroot.clone()
            } else {
                build_path_in_chroot.join(rel)
            };

            let _ = std::fs::create_dir_all(selected_build.path.join("build"));
            let build_dir_chroot = src_dir_chroot.join("build");

            let run = |command: &str, args: Vec<String>, cwd: Option<&Path>| -> bool {
                match run_chroot_command(&chroot_env, command, &args, cwd) {
                    Ok(exit_status) => exit_status.success(),
                    Err(e) => {
                        eprintln!("{} {}: {}", "Command failed".red(), command, e);
                        false
                    }
                }
            };

            let mut build_successful = false;
            let mut install_successful = false;
            match selected_build.kind {
                BuildSystemKind::Cargo => {
                    pb_build.set_message("Building with 'cargo' in chroot...");
                    let mut args = vec!["build".to_string(), "--release".to_string()];
                    args.extend(profile.build_args.clone());
                    build_successful = run("cargo", args, Some(&src_dir_chroot));
                    if build_successful {
                        pb_build.set_message("Installing with 'cargo' in chroot...");
                        let mut install = vec![
                            "install".to_string(),
                            "--path".to_string(),
                            src_dir_chroot.to_string_lossy().to_string(),
                            "--root".to_string(),
                            staging_dir_in_chroot.to_string_lossy().to_string(),
                        ];
                        install.extend(profile.install_args.clone());
                        install_successful = run("cargo", install, None);
                    }
                }
                BuildSystemKind::Meson => {
                    pb_build.set_message("Configuring with 'meson' in chroot...");
                    let mut setup_args = vec![
                        "setup".to_string(),
                        build_dir_chroot.to_string_lossy().to_string(),
                        src_dir_chroot.to_string_lossy().to_string(),
                        "--prefix=/usr".to_string(),
                    ];
                    setup_args.extend(profile.configure_args.clone());
                    if run("meson", setup_args, None) {
                        pb_build.set_message("Building with 'meson' in chroot...");
                        let mut compile_args = vec![
                            "compile".to_string(),
                            "-C".to_string(),
                            build_dir_chroot.to_string_lossy().to_string(),
                        ];
                        compile_args.extend(profile.build_args.clone());
                        build_successful = run("meson", compile_args, None);
                        if build_successful {
                            pb_build.set_message("Installing with 'meson' in chroot...");
                            let mut install_args_vec = vec![
                                "install".to_string(),
                                "-C".to_string(),
                                build_dir_chroot.to_string_lossy().to_string(),
                                "--destdir".to_string(),
                                staging_dir_in_chroot.to_string_lossy().to_string(),
                            ];
                            install_args_vec.extend(profile.install_args.clone());
                            install_successful = run("meson", install_args_vec, None);
                        }
                    }
                }
                BuildSystemKind::Cmake => {
                    pb_build.set_message("Configuring with 'cmake' in chroot...");
                    let mut cmake_args = vec![
                        "-S".to_string(),
                        src_dir_chroot.to_string_lossy().to_string(),
                        "-B".to_string(),
                        build_dir_chroot.to_string_lossy().to_string(),
                        "-DCMAKE_BUILD_TYPE=Release".to_string(),
                        "-DCMAKE_INSTALL_PREFIX=/usr".to_string(),
                    ];
                    cmake_args.extend(profile.configure_args.clone());
                    if run("cmake", cmake_args, None) {
                        pb_build.set_message("Building with 'cmake' in chroot...");
                        let mut build_args_vec = vec![
                            "--build".to_string(),
                            build_dir_chroot.to_string_lossy().to_string(),
                        ];
                        if !profile.build_args.is_empty() {
                            build_args_vec.push("--".to_string());
                            build_args_vec.extend(profile.build_args.clone());
                        }
                        build_successful = run("cmake", build_args_vec, None);
                        if build_successful {
                            pb_build.set_message("Installing with 'cmake' in chroot...");
                            let mut install_args_vec = vec![
                                format!("DESTDIR={}", staging_dir_in_chroot.to_string_lossy()),
                                "cmake".to_string(),
                                "--install".to_string(),
                                build_dir_chroot.to_string_lossy().to_string(),
                                "--prefix".to_string(),
                                "/usr".to_string(),
                            ];
                            install_args_vec.extend(profile.install_args.clone());
                            install_successful = run("env", install_args_vec, None);
                        }
                    }
                }
                BuildSystemKind::Scons => {
                    pb_build.set_message("Building with 'scons' in chroot...");
                    let args = profile.build_args.clone();
                    build_successful = run("scons", args, Some(&src_dir_chroot));
                    if build_successful {
                        pb_build.set_message("Installing with 'scons' in chroot...");
                        let mut install = vec![
                            "install".to_string(),
                            format!("DESTDIR={}", staging_dir_in_chroot.to_string_lossy()),
                            "PREFIX=/usr".to_string(),
                        ];
                        install.extend(profile.install_args.clone());
                        install_successful = run("scons", install, Some(&src_dir_chroot));
                    }
                }
                BuildSystemKind::Make => {
                    let configure_script = selected_build.path.join("configure");
                    if configure_script.exists() {
                        pb_build.set_message("Running configure script in chroot...");
                        let mut cfg_args = vec!["--prefix=/usr".to_string()];
                        cfg_args.extend(profile.configure_args.clone());
                        if !run("./configure", cfg_args, Some(&src_dir_chroot)) {
                            build_successful = false;
                            pb_build.finish_with_message("Configure step failed.".red().to_string());
                        }
                    }

                    if !pb_build.is_finished() {
                        pb_build.set_message("Building with 'make' in chroot...");
                        let args = profile.build_args.clone();
                        build_successful = run("make", args, Some(&src_dir_chroot));
                        if build_successful {
                            pb_build.set_message("Installing with 'make' in chroot...");
                            let mut install = vec![
                                "install".to_string(),
                                format!("DESTDIR={}", staging_dir_in_chroot.to_string_lossy()),
                                "PREFIX=/usr".to_string(),
                            ];
                            install.extend(profile.install_args.clone());
                            install_successful = run("make", install, Some(&src_dir_chroot));
                        }
                    }
                }
            }

            if build_successful && install_successful {
                pb_build.set_message("Packaging artifacts...");
                let recipe = build_recipe(&package_name, &package_version, selected_build.kind, &profile);
                match buildpkg::create_package(&chroot_path, &staging_dir_in_chroot, &output_dir, &recipe) {
                    Ok(path) => {
                        pb_build.finish_with_message(format!("Packaged {} -> {}", package_name, path.display()).green().to_string());
                    }
                    Err(e) => {
                        pb_build.finish_with_message(format!("Packaging failed: {}", e).red().to_string());
                    }
                }
            } else if build_successful && !install_successful {
                pb_build.finish_with_message(format!("Install failed for {}.", package_name).red().to_string());
            } else if !pb_build.is_finished() {
                pb_build.finish_with_message(format!("Build process for {} failed.", package_name).red().to_string());
            }

            // --- Chroot Cleanup ---
            if let Err(e) = chroot_env.cleanup() {
                eprintln!("{} {}", "Warning: Failed to cleanup chroot environment:".yellow(), e);
            }

        }

        Commands::RepoRemote { action } => {
            match action {
                RepoRemoteAction::List => {
                    let cfg_now = AppConfig::load();
                    let active = cfg_now.active_repo.clone();
                    if cfg_now.repo_remotes.is_empty() {
                        println!("{}", "No binary repo remotes configured.".yellow());
                    } else {
                        println!("Configured binary repo remotes ({}):", cfg_now.repo_remotes.len());
                        for (name, url) in cfg_now.repo_remotes.iter() {
                            if Some(name.clone()) == active {
                                println!("* {} -> {} {}", name.cyan(), url, "(active)".green());
                            } else {
                                println!("  {} -> {}", name.cyan(), url);
                            }
                        }
                    }
                }
                RepoRemoteAction::Add { name, url } => {
                    match AppConfig::add_repo_remote(&name, &url) {
                        Ok(_) => println!("{} {} -> {}", "Added/updated binary remote:".green(), name, url),
                        Err(e) => eprintln!("{} {}", "Failed to add remote:".red(), e),
                    }
                }
                RepoRemoteAction::Remove { name } => {
                    match AppConfig::remove_repo_remote(&name) {
                        Ok(_) => println!("{} {}", "Removed binary remote:".green(), name),
                        Err(e) => eprintln!("{} {}", "Failed to remove remote:".red(), e),
                    }
                }
                RepoRemoteAction::Choose { name } => {
                    match AppConfig::set_active_repo(&name) {
                        Ok(_) => {
                            let cfg_now = AppConfig::load();
                            println!("Active binary remote set to '{}' -> {}", name.cyan(), cfg_now.repo_url);
                        }
                        Err(e) => eprintln!("{} {}", "Failed to set active remote:".red(), e),
                    }
                }
                RepoRemoteAction::Current => {
                    let cfg_now = AppConfig::load();
                    println!("{}", cfg_now.repo_url);
                }
            }
        }

        Commands::Repos { action } => {
            match action {
                RepoAction::List => {
                    let list = repo::configured_repos();
                    if list.is_empty() { println!("{}", "No configured repositories.".yellow()); }
                    else {
                        println!("Configured repositories ({}):", list.len());
                        for r in list { println!("- {} -> {}", r.name.cyan(), r.clone_url); }
                    }
                }
                RepoAction::Add { name, url } => {
                    match repo::add_repo_entry(&name, &url) {
                        Ok(_) => println!("{} {} -> {}", "Added/updated:".green(), name, url),
                        Err(e) => eprintln!("{} {}", "Failed to add repo:".red(), e),
                    }
                }
                RepoAction::Remove { name } => {
                    match repo::remove_repo_entry(&name) {
                        Ok(_) => println!("{} {}", "Removed:".green(), name),
                        Err(e) => eprintln!("{} {}", "Failed to remove repo:".red(), e),
                    }
                }
                RepoAction::Choose { term, build, print_url } => {
                    match repo::select_repo_from_config(term.as_deref()) {
                        Ok(selected) => {
                            println!("Selected: {} -> {}", selected.name.cyan(), selected.clone_url);
                            if print_url { println!("{}", selected.clone_url); }
                            if build {
                                println!("{} {}", "Tip:".yellow(), format!("Run: nxpkg buildins '{}'", selected.name));
                            }
                        }
                        Err(e) => eprintln!("{} {}", "Selection failed:".red(), e),
                    }
                }
            }
        }

        Commands::Debug1 { name} => {
            match compress::decompress_tarball(&name) {
                Ok(_) => {
                    println!("{} package is decompressed!", &name);
                }
                Err(e) => {
                    eprintln!("FAIL: {} package is not extracted!: {}", &name, e);
                }
            }
        }
        Commands::About => {
            println!("{}", "NeoniX PacKaGe Manager for Neonix v1.x".blue());
            println!("{}", "This is designed especially for Neonix family Linux distro. Compact and community oriented.".yellow());
        }
        Commands::Version => {
            println!("Neonix {} ({})", VERSION, std::env::consts::ARCH);
        }
        Commands::Health { no_network, check_chroot } => {
            let pb = ProgressBar::new_spinner();
            pb.enable_steady_tick(std::time::Duration::from_millis(120));
            pb.set_style(ProgressStyle::with_template("{spinner:.green} {elapsed_precise} {msg}").unwrap());
            pb.set_message("Running health checks...");

            let mut ok = true;

            // 1) Database check: ensure we can query the packages table
            match db1.db.query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='packages'",
                [],
                |row| row.get::<_, String>(0),
            ) {
                Ok(_name) => {}
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    ok = false;
                    eprintln!("{} {}", "DB check failed:".red(), "packages table missing");
                }
                Err(e) => {
                    ok = false;
                    eprintln!("{} {}", "DB check failed:".red(), e);
                }
            }

            // 2) Cache dir write test
            let tmp_file = cfg.cache_dir.join(".nxpkg_healthcheck.tmp");
            match std::fs::write(&tmp_file, b"ok") {
                Ok(_) => { let _ = std::fs::remove_file(&tmp_file); }
                Err(e) => { ok = false; eprintln!("{} {}", "Cache dir write failed:".red(), e); }
            }

            // 3) Network + repo index (unless skipped)
            if !no_network {
                match download::fetch_index_verified(&cfg.repo_url, Some(&cfg.pubkey_path), cfg.require_signed_index).await {
                    Ok(_) => {}
                    Err(e) => { ok = false; eprintln!("{} {}", "Repo index fetch failed:".red(), e); }
                }
            }

            // 4) Optional chroot prerequisites: presence of needed tools
            if check_chroot {
                let tools = [
                    "bash", "sh", "make", "gcc", "g++", "cargo", "meson",
                    "ninja", "cmake", "git", "scons", "python", "ld"
                ];
                for t in tools.iter() {
                    let status = std::process::Command::new("which").arg(t).status();
                    if status.map_or(true, |s| !s.success()) {
                        ok = false;
                        eprintln!("{} '{}' not found in PATH", "Missing tool:".red(), t);
                    }
                }
            }

            if ok {
                pb.finish_with_message("Health OK".green().to_string());
            } else {
                pb.finish_with_message("Health check failed".red().to_string());
                std::process::exit(1);
            }
        }
        Commands::Publish { file, desc, repo, token, sign_keypair_b64, sign_keypair_file } => {
            let nxpkg_path = PathBuf::from(&file);
            if !nxpkg_path.exists() {
                eprintln!("{}", format!("Package file not found: {}", nxpkg_path.display()).red());
                return;
            }
            // Determine repo URL
            let repo_url = repo.unwrap_or_else(|| cfg.repo_url.clone());
            // Determine token
            let token_effective = token
                .or_else(|| std::env::var("NXPKG_TOKEN").ok());
            // Determine signing keypair
            let keypair_b64 = if let Some(p) = sign_keypair_file {
                match std::fs::read_to_string(p) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        eprintln!("{}", format!("Failed to read sign keypair file: {}", e).red());
                        return;
                    }
                }
            } else {
                sign_keypair_b64.or_else(|| std::env::var("NXPKG_SIGN_KEYPAIR_B64").ok())
            };

            // Read recipe (without installing)
            let recipe = match compress::read_recipe_from_nxpkg(&nxpkg_path) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{}", format!("Failed to read recipe from package: {}", e).red());
                    return;
                }
            };

            let pb = ProgressBar::new_spinner();
            pb.enable_steady_tick(std::time::Duration::from_millis(120));
            pb.set_style(ProgressStyle::with_template("{spinner:.green} {elapsed_precise} {msg}").unwrap());
            pb.set_message("Uploading package and updating index...");

            match upload::upload_and_update_index(
                &repo_url,
                &nxpkg_path,
                &recipe,
                desc.as_deref(),
                token_effective.as_deref(),
                keypair_b64.as_deref(),
            ).await {
                Ok(_) => pb.finish_with_message("Publish complete".green().to_string()),
                Err(e) => pb.finish_with_message(format!("Publish failed: {}", e).red().to_string()),
            }
        }
    }
}
