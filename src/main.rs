mod db;
mod compress;
mod buildins;
mod repo;
use crate::db::download;

pub use compress::decompress_tarball;
pub use db::PackageManagerDB;
use clap::{Parser, Subcommand};
use rusqlite::{Connection, types::Null};
use indicatif::{ProgressBar, ProgressStyle};
use colored::*;
// Indicates version of the nxpkg source code for every ".rs" file
pub const VERSION: &str = "v0.1.0";

/// info
#[derive(Parser)]
#[command(name = "nxpkg")]
#[command(about = "NeoniX PacKaGe Manager for Neonix v1.x \n
Version 1.0 \n
This is designed especially for Neonix family Linux distro. Compact and community oriented. \n

MIT License \n \n

Copyright (c) 2025 Efe Ilhan Yuce \n \n

Permission is hereby granted, free of charge, to any person obtaining a copy \n
of this software and associated documentation files (the Software), to deal \n
in the Software without restriction, including without limitation the rights \n
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell \n
copies of the Software, and to permit persons to whom the Software is \n
furnished to do so, subject to the following conditions: \n \n

The above copyright notice and this permission notice shall be included in all \n
copies or substantial portions of the Software. \n \n

THE SOFTWARE IS PROVIDED AS IS, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR \n
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, \n
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE \n
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER \n
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, \n
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE \n
SOFTWARE.
")]

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

// The repository URL. In a real application, this would come from a config file.
const REPO_URL: &str = "https://your-server.com/releases"; // <-- DEĞİŞTİRİN

#[tokio::main]
async fn main() {
    let path = "nxpkg_meta.db";
    let cli = Cli::parse();
    let Some(val) = Connection::open(path).ok() else { return };
    let db1 = match PackageManagerDB::new(path) {
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
                
                let index = match download::fetch_index(REPO_URL) {
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
                nxpkg_path = PathBuf::from(format!("/tmp/{}.nxpkg", package_name_from_source));

                pb.finish_and_clear();
                
                if let Err(e) = download::download_file_with_progress(&package_entry.download_url, &nxpkg_path).await {
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

            let index = match download::fetch_index(REPO_URL) {
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
            pb_build.set_message(format!("Detecting build system for {}...", selected_repo.name));

            let mut build_successful = false;

            match find_build_system(clone_path_obj) {
                Some(BuildSystem::Cargo(path)) => {
                    pb_build.set_message("Building with 'cargo'...");
                    let status = pb_build.suspend(|| {
                        Command::new("cargo").arg("build").arg("--release").current_dir(path).status()
                    });
                    if let Ok(exit_status) = status { build_successful = exit_status.success(); }
                }
                Some(BuildSystem::Meson(path)) => {
                    pb_build.set_message("Configuring with 'meson'...");
                    let build_dir = path.join("build");
                    let _ = std::fs::create_dir(&build_dir);

                    let setup_status = pb_build.suspend(|| {
                        Command::new("meson").arg("setup").arg(&build_dir).current_dir(&path).status()
                    });
                    
                    if setup_status.map_or(false, |s| s.success()) {
                        pb_build.set_message("Building with 'ninja'...");
                        let build_status = pb_build.suspend(|| {
                            Command::new("ninja").current_dir(&build_dir).status()
                        });
                        if let Ok(exit_status) = build_status { build_successful = exit_status.success(); }
                    } else {
                        pb_build.finish_with_message("Meson setup failed.".red().to_string());
                    }
                }
                Some(BuildSystem::CMake(path)) => {
                    pb_build.set_message("Configuring with 'cmake'...");
                    let build_dir = path.join("build");
                    let _ = std::fs::create_dir(&build_dir);
                    
                    let cmake_status = pb_build.suspend(|| {
                        Command::new("cmake").arg(&path).current_dir(&build_dir).status()
                    });

                    if cmake_status.map_or(false, |s| s.success()) {
                        pb_build.set_message("Building with 'make'...");
                        let make_status = pb_build.suspend(|| {
                             Command::new("make").current_dir(&build_dir).status()
                        });
                        if let Ok(exit_status) = make_status { build_successful = exit_status.success(); }
                    } else {
                        pb_build.finish_with_message("CMake configuration failed.".red().to_string());
                    }
                }
                Some(BuildSystem::SCons(path)) => {
                    pb_build.set_message("Building with 'scons'...");
                    let status = pb_build.suspend(|| {
                        Command::new("scons").current_dir(path).status()
                    });
                    if let Ok(exit_status) = status { build_successful = exit_status.success(); }
                }
                Some(BuildSystem::Make(path)) => {
                    pb_build.set_message("Building with 'make'...");
                    let status = pb_build.suspend(|| {
                        Command::new("make").current_dir(path).status()
                    });
                    if let Ok(exit_status) = status { build_successful = exit_status.success(); }
                }
                None => {
                    pb_build.finish_with_message(format!("Could not detect a known build system in {}.", selected_repo.name).red().to_string());
                }
            }

            if build_successful {
                pb_build.finish_with_message(format!("Successfully built {}!", selected_repo.name).green().to_string());
                println!("Package ready at: {}", clone_path);
            } else if !pb_build.is_finished() {
                pb_build.finish_with_message(format!("Build process for {} failed.", selected_repo.name).red().to_string());
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
            println!("{}", "\n{}\n\nCopyright (c) 2025 Efe Ilhan Yuce\n\nPermission is hereby granted, free of charge, to any person obtaining a copyof this software and associated documentation files (the Software), to dealin the Software without restriction, including without limitation the rights\nto use, copy, modify, merge, publish, distribute, sublicense, and/or sell\ncopies of the Software, and to permit persons to whom the Software isfurnished to do so, subject to the following conditions:\n\nThe above copyright notice and this permission notice shall be included in allcopies or substantial portions of the Software.\n\nTHE SOFTWARE IS PROVIDED AS IS, WITHOUT WARRANTY OF ANY KIND, EXPRESS ORIMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THEAUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHERLIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THESOFTWARE.".red());
        }
        Commands::Version => {
            println!("Neonix {} ({})", VERSION, std::env::consts::ARCH);
        }
    }
}