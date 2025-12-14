mod db;
mod compress;
mod buildins;
mod repo;
mod config;
mod trust;
use crate::db::download;
use crate::db::upload;
use crate::buildins::chroot::ChrootEnv;
use crate::config::AppConfig;
use std::fs;


pub use compress::decompress_tarball;
pub use db::PackageManagerDB;
use clap::{Parser, Subcommand};
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
        /// Package name
        name: String,
    },

    // Show version of the nxpkg
    Version,

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

// Helper enum and function for build system detection
use walkdir::WalkDir;
use std::path::{Path, PathBuf};

#[derive(Debug)]
enum BuildSystem {
    Cargo(PathBuf),
    Meson(PathBuf),
    CMake(PathBuf),
    SCons(PathBuf),
    Make(PathBuf),
}

/// Recursively finds the best build system in the cloned repository.
fn find_build_system(root_path: &Path) -> Option<BuildSystem> {
    let mut candidates: Vec<BuildSystem> = Vec::new();

    for entry in WalkDir::new(root_path).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if path.is_file() {
            match path.file_name().and_then(|s| s.to_str()) {
                Some("Cargo.toml") => candidates.push(BuildSystem::Cargo(path.parent().unwrap().to_path_buf())),
                Some("meson.build") => candidates.push(BuildSystem::Meson(path.parent().unwrap().to_path_buf())),
                Some("CMakeLists.txt") => candidates.push(BuildSystem::CMake(path.parent().unwrap().to_path_buf())),
                Some("SConstruct") => candidates.push(BuildSystem::SCons(path.parent().unwrap().to_path_buf())),
                Some("Makefile") => candidates.push(BuildSystem::Make(path.parent().unwrap().to_path_buf())),
                _ => {}
            }
        }
    }

    // Return the best candidate based on a priority list
    candidates.into_iter().min_by_key(|c| match c {
        BuildSystem::Cargo(_) => 0,
        BuildSystem::Meson(_) => 1,
        BuildSystem::SCons(_) => 2,
        BuildSystem::CMake(_) => 3,
        BuildSystem::Make(_) => 4,
    })
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
                
                package_name_from_source = remote_name;
                nxpkg_path = cfg.cache_dir.join(format!("{}.nxpkg", package_name_from_source));

                pb.finish_and_clear();
                
                if let Err(e) = download::download_file_with_progress(&package_entry.download_url, &nxpkg_path, package_entry.sha256.as_deref()).await {
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
            let (recipe, _installed_files) = match compress::extract_nxpkg(&nxpkg_path) {
                Ok(r) => r,
                Err(e) => {
                    pb.finish_with_message(format!("Failed to install package: {}", e).red().to_string());
                    return;
                }
            };
            
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
        Commands::Buildins { name } => {
            let selected_repo = match repo::find_and_select_repo(&name) {
                Ok(repo) => repo,
                Err(e) => {
                    eprintln!("{}", format!("\nBuild process failed: {}", e).red());
                    return;
                }
            };
            
            println!("\nProceeding to build '{}'.", selected_repo.name.cyan());

            use std::process::Command;

            let pb_clone = ProgressBar::new_spinner();
            pb_clone.enable_steady_tick(std::time::Duration::from_millis(120));
            pb_clone.set_style(ProgressStyle::with_template("{spinner:.green} {elapsed_precise} {msg}").unwrap());
            
            let repo_name_only = selected_repo.name.split('/').last().unwrap_or(&selected_repo.name);
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
            let new_repo_path = chroot_build_dir.join(repo_name_only);
            if let Err(e) = std::fs::rename(&clone_path, &new_repo_path) {
                 pb_build.finish_with_message(format!("Failed to move repo into chroot: {}", e).red().to_string());
                let _ = chroot_env.cleanup();
                return;
            }

            pb_build.set_message(format!("Detecting build system for {} inside chroot...", selected_repo.name));

            let mut build_successful = false;
            
            // The path inside the chroot is different
            let build_path_in_chroot = Path::new("/build").join(repo_name_only);

            match find_build_system(&new_repo_path) { // Detect on the real path
                Some(BuildSystem::Cargo(_)) => {
                    pb_build.set_message("Building with 'cargo' in chroot...");
                    let status = chroot_env.run_command(
                        "/usr/bin/cargo", 
                        &["build", "--release", "--manifest-path", &build_path_in_chroot.join("Cargo.toml").to_string_lossy()]
                    );
                    if let Ok(exit_status) = status { build_successful = exit_status.success(); }
                }
                Some(BuildSystem::Meson(path)) => {
                    // Meson needs to be handled differently inside chroot
                    pb_build.set_message("Building with 'meson/ninja' in chroot...");
                     let status = chroot_env.run_command("/bin/bash", &[
                        "-c", 
                        &format!("cd {} && meson setup build && ninja -C build", build_path_in_chroot.display())
                    ]);
                    if let Ok(exit_status) = status { build_successful = exit_status.success(); }
                }
                 Some(BuildSystem::CMake(path)) => {
                    pb_build.set_message("Building with 'cmake/make' in chroot...");
                    let status = chroot_env.run_command("/bin/bash", &[
                        "-c", 
                        &format!("cd {} && cmake . && make", build_path_in_chroot.display())
                    ]);
                    if let Ok(exit_status) = status { build_successful = exit_status.success(); }
                }
                Some(BuildSystem::SCons(path)) => {
                    pb_build.set_message("Building with 'scons' in chroot...");
                    let status = chroot_env.run_command("/bin/bash", &[
                        "-c", 
                        &format!("cd {}", build_path_in_chroot.display())
                    ]);
                    if let Ok(exit_status) = status { build_successful = exit_status.success(); }
                }
                Some(BuildSystem::Make(path)) => {
                    pb_build.set_message("Building with 'make' in chroot...");
                     let status = chroot_env.run_command("/bin/bash", &[
                        "-c", 
                        &format!("cd {} && make", build_path_in_chroot.display())
                    ]);
                    if let Ok(exit_status) = status { build_successful = exit_status.success(); }
                }
                None => {
                    pb_build.finish_with_message(format!("Could not detect a known build system in {}.", selected_repo.name).red().to_string());
                }
            }

            if build_successful {
                pb_build.finish_with_message(format!("Successfully built {}!", selected_repo.name).green().to_string());
                println!("Package artifacts are available in the chroot environment (temporarily).");
                // Next step: buildpkg.rs would take over here to package the artifacts.
            } else if !pb_build.is_finished() {
                pb_build.finish_with_message(format!("Build process for {} failed.", selected_repo.name).red().to_string());
            }

            // --- Chroot Cleanup ---
            if let Err(e) = chroot_env.cleanup() {
                eprintln!("{} {}", "Warning: Failed to cleanup chroot environment:".yellow(), e);
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