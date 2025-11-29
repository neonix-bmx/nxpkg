use flate2::read::GzDecoder;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use tar::Archive;
use walkdir::WalkDir;
use crate::buildins::meta::PackageRecipe; // Import the recipe struct

/// A generic helper function to extract any .tar.gz file to a specified destination.
pub fn extract_tar_gz(source_file: &Path, dest_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !source_file.exists() {
        return Err(format!("Source file not found: {}", source_file.display()).into());
    }

    fs::create_dir_all(dest_dir)?;
    let file = File::open(source_file)?;
    let reader = BufReader::new(file);
    let decompressor = GzDecoder::new(reader);
    let mut archive = Archive::new(decompressor);
    archive.unpack(dest_dir)?;
    
    Ok(())
}

/// Extracts a .nxpkg, parses its recipe, and installs files to their final destinations.
///
/// Returns a tuple containing:
/// 1. The parsed `PackageRecipe`.
/// 2. A `Vec<PathBuf>` of the absolute paths of the installed files.
pub fn extract_nxpkg(nxpkg_path: &Path) -> Result<(PackageRecipe, Vec<PathBuf>), Box<dyn std::error::Error>> {
    // Stage 1: Extract the .nxpkg container to a temporary location.
    let stage1_dir = PathBuf::from("/tmp/nxpkg_stage1");
    if stage1_dir.exists() {
        fs::remove_dir_all(&stage1_dir)?;
    }
    extract_tar_gz(nxpkg_path, &stage1_dir)?;

    // Stage 2: Parse the recipe file from the extracted contents.
    let recipe_path = stage1_dir.join("package.cfg");
    if !recipe_path.exists() {
        return Err("Invalid .nxpkg: 'package.cfg' not found.".into());
    }
    let recipe = PackageRecipe::from_file(&recipe_path)
        .map_err(|e| format!("Failed to parse package.cfg: {}", e))?;

    // Stage 3: Extract the data.tar.gz to a *second* temporary location (stage2).
    let data_tarball_path = stage1_dir.join("data.tar.gz");
    if !data_tarball_path.exists() {
        return Err("Invalid .nxpkg: 'data.tar.gz' not found.".into());
    }
    let stage2_dir = PathBuf::from("/tmp/nxpkg_stage2");
    if stage2_dir.exists() {
        fs::remove_dir_all(&stage2_dir)?;
    }
    extract_tar_gz(&data_tarball_path, &stage2_dir)?;

    // Stage 4: Walk the stage2 directory and copy files to their final destination.
    let mut final_installed_paths = Vec::new();
    for entry in WalkDir::new(&stage2_dir).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_file() {
            let temp_path = entry.path();
            let relative_path = temp_path.strip_prefix(&stage2_dir)?;
            
            // Prevent directory traversal attacks.
            if relative_path.components().any(|c| c == std::path::Component::ParentDir) {
                 return Err(format!("Aborting installation: package contains potentially malicious path '..': {}", relative_path.display()).into());
            }

            let dest_path = PathBuf::from("/").join(relative_path);

            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            
            fs::copy(temp_path, &dest_path)?;
            final_installed_paths.push(dest_path);
        }
    }
    
    // Stage 5: Clean up temporary directories.
    fs::remove_dir_all(&stage1_dir)?;
    fs::remove_dir_all(&stage2_dir)?;

    Ok((recipe, final_installed_paths))
}

// Keep the old function for compatibility with the Debug1 command, but have it use the new helper.
pub fn decompress_tarball(input_file: &str) -> Result<(), Box<dyn std::error::Error>> {
    let input_path = Path::new("/tmp/").join(format!("{}.tar.gz", input_file));
    let dest_dir = Path::new("/tmp/nxpkg_extract");
    extract_tar_gz(&input_path, dest_dir)
}
