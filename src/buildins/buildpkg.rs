//! src/buildins/buildpkg.rs
//! This module handles packaging the build artifacts from a chroot environment into a .nxpkg file.

use std::path::{Path, PathBuf};
use crate::compress; // Accessing the functions from the top-level compress module
use crate::buildins::meta::PackageRecipe; // Use the PackageRecipe defined in buildins::meta

/// Creates a .nxpkg package from a staging directory within the chroot.
///
/// # Arguments
/// * `chroot_path` - The root of the chroot environment.
/// * `staging_dir_in_chroot` - The path *inside* the chroot where artifacts were installed (e.g., "/pkg").
/// * `output_dir` - Where to save the final .nxpkg file.
/// * `recipe` - The package metadata.
///
/// # Returns
/// The path to the created .nxpkg file.
pub fn create_package(
    chroot_path: &Path,
    staging_dir_in_chroot: &Path,
    output_dir: &Path,
    recipe: &PackageRecipe,
) -> Result<PathBuf, String> {
    println!("Packaging build artifacts into a .nxpkg file...");

    let staging_path = chroot_path.join(staging_dir_in_chroot.strip_prefix("/").unwrap());
    
    if !staging_path.exists() || !staging_path.is_dir() {
        return Err(format!(
            "Staging directory '{}' does not exist inside the chroot.",
            staging_path.display()
        ));
    }

    // 1. Create the final .nxpkg file path
    let output_filename = format!(
        "{}-{}.nxpkg",
        recipe.package.name, recipe.package.version
    );
    let output_filepath = output_dir.join(&output_filename);
    
    // 2. Use the existing compress::create_nxpkg function
    // This function will handle creating data.tar.gz from the staging path and packaging
    // it with the recipe.
    match compress::create_nxpkg(&staging_path, recipe, &output_filepath) {
        Ok(_) => {
            println!(
                "Successfully created package: {}",
                output_filepath.display()
            );
            Ok(output_filepath)
        }
        Err(e) => Err(format!("Failed to create .nxpkg archive: {}", e)),
    }
}

