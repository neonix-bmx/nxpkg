use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tar::{Archive, Builder};
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

    // Stage 2.5: Architecture validation BEFORE installing anything.
    let supports_current_arch = {
        // Normalize function to compare arch names
        fn norm(s: &str) -> String { s.trim().to_lowercase().replace('-', "_") }
        // Accept if recipe declares no architectures (means universal), or explicitly allows "any"/"noarch".
        if recipe.package.architectures.is_empty() {
            true
        } else {
            let declared: Vec<String> = recipe.package.architectures.iter().map(|s| norm(s)).collect();
            let mut aliases: Vec<&'static str> = match std::env::consts::ARCH {
                "x86_64" => vec!["x86_64", "amd64", "x64"],
                "aarch64" => vec!["aarch64", "arm64"],
                "arm" => vec!["arm", "armv7", "armhf", "armv7l"],
                "x86" | "i686" => vec!["x86", "i686", "i386"],
                "powerpc64" | "powerpc64le" => vec!["ppc64", "ppc64le"],
                other => vec![other],
            };
            // Also treat universal tokens as valid
            aliases.extend(["any", "noarch"].iter().copied());
            let aliases: Vec<String> = aliases.into_iter().map(|s| s.to_string()).collect();
            declared.iter().any(|d| aliases.iter().any(|a| a == d))
        }
    };

    if !supports_current_arch {
        // Clean up stage1 because we won't proceed
        let _ = fs::remove_dir_all(&stage1_dir);
        return Err(format!(
            "Package is not built for this architecture (host: {}, package: {:?})",
            std::env::consts::ARCH,
            recipe.package.architectures
        ).into());
    }

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

/// Creates a .nxpkg archive from a staging directory and a recipe file.
/// The resulting archive contains two entries:
/// - package.cfg (the recipe in INI-like format)
/// - data.tar.gz (tarball of the staged filesystem)
pub fn create_nxpkg(staging_dir: &Path, recipe: &PackageRecipe, output_path: &Path) -> Result<(), String> {
    if !staging_dir.is_dir() {
        return Err(format!("Staging directory does not exist or is not a directory: {}", staging_dir.display()));
    }

    // 1) Build data.tar.gz from the staging directory
    let tmp_dir = std::env::temp_dir().join("nxpkg_pack");
    if tmp_dir.exists() {
        fs::remove_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;

    let data_tar_gz_path = tmp_dir.join("data.tar.gz");
    {
        let data_file = File::create(&data_tar_gz_path).map_err(|e| e.to_string())?;
        let enc = GzEncoder::new(data_file, Compression::default());
        let mut tar_builder = Builder::new(enc);

        // Add directories and files preserving relative paths
        for entry in WalkDir::new(staging_dir).into_iter().filter_map(Result::ok) {
            let rel = entry.path().strip_prefix(staging_dir).map_err(|e| e.to_string())?;
            if rel.as_os_str().is_empty() {
                continue;
            }
            if entry.file_type().is_dir() {
                tar_builder.append_dir(rel, entry.path()).map_err(|e| e.to_string())?;
            } else if entry.file_type().is_file() {
                tar_builder.append_path_with_name(entry.path(), rel).map_err(|e| e.to_string())?;
            }
        }
        // Finalize encoder
        let enc = tar_builder.into_inner().map_err(|e| e.to_string())?;
        enc.finish().map_err(|e| e.to_string())?;
    }

    // 2) Render package.cfg content from the recipe
    let cfg = {
        let mut s = String::new();
        s.push_str("[package]\n");
        s.push_str(&format!("name = {}\n", recipe.package.name));
        s.push_str(&format!("version = {}\n", recipe.package.version));
        if !recipe.package.architectures.is_empty() {
            s.push_str(&format!(
                "architectures = {}\n",
                recipe.package.architectures.join(", ")
            ));
        }
        s.push_str("\n[build]\n");
        if !recipe.build.dependencies.is_empty() {
            s.push_str(&format!(
                "dependencies = {}\n",
                recipe.build.dependencies.join(", ")
            ));
        }
        if !recipe.build.commands.is_empty() {
            s.push_str(&format!(
                "commands = {}\n",
                recipe.build.commands.join("; ")
            ));
        }
        s.push_str("\n[install]\n");
        if !recipe.install.install_params.is_empty() {
            s.push_str(&format!(
                "install_params = {}\n",
                recipe.install.install_params.join(", ")
            ));
        }
        s
    };

    // 3) Create the final .nxpkg tar archive
    {
        let mut outer = Builder::new(File::create(output_path).map_err(|e| e.to_string())?);

        // Append package.cfg
        let cfg_bytes = cfg.as_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_size(cfg_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        outer.append_data(&mut header, "package.cfg", cfg_bytes).map_err(|e| e.to_string())?;

        // Append data.tar.gz
        let mut header = tar::Header::new_gnu();
        let data_meta = fs::metadata(&data_tar_gz_path).map_err(|e| e.to_string())?;
        header.set_size(data_meta.len());
        header.set_mode(0o644);
        header.set_cksum();
        let data_file = File::open(&data_tar_gz_path).map_err(|e| e.to_string())?;
        outer.append_file("data.tar.gz", &mut data_file.try_clone().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;

        outer.finish().map_err(|e| e.to_string())?;
    }

    // 4) Cleanup temporary artifacts
    if let Err(e) = fs::remove_dir_all(&tmp_dir) { eprintln!("Warning: could not clean temp dir {}: {}", tmp_dir.display(), e); }

    Ok(())
}

/// Read only the package.cfg (recipe) from a .nxpkg without installing anything.
/// Supports both plain tar and gzipped outer container.
pub fn read_recipe_from_nxpkg(nxpkg_path: &Path) -> Result<PackageRecipe, Box<dyn std::error::Error>> {
    let mut file = File::open(nxpkg_path)?;
    let mut magic = [0u8; 2];
    let _ = file.read(&mut magic)?;
    file.seek(SeekFrom::Start(0))?;

    // Decide reader based on gzip magic
    let recipe_string = if magic == [0x1f, 0x8b] {
        let dec = GzDecoder::new(file);
        let mut archive = Archive::new(dec);
        let mut recipe_content = String::new();
        for entry in archive.entries()? {
            let mut entry = entry?;
            if entry.path()?.as_ref() == Path::new("package.cfg") {
                entry.read_to_string(&mut recipe_content)?;
                break;
            }
        }
        if recipe_content.is_empty() { return Err("package.cfg not found in .nxpkg".into()); }
        recipe_content
    } else {
        let mut archive = Archive::new(file);
        let mut recipe_content = String::new();
        for entry in archive.entries()? {
            let mut entry = entry?;
            if entry.path()?.as_ref() == Path::new("package.cfg") {
                entry.read_to_string(&mut recipe_content)?;
                break;
            }
        }
        if recipe_content.is_empty() { return Err("package.cfg not found in .nxpkg".into()); }
        recipe_content
    };

    // Parse by writing to a temporary file and reusing the existing parser
    let tmp_path = std::env::temp_dir().join(format!("nxpkg_pkgcfg_{}.cfg", std::process::id()));
    fs::write(&tmp_path, recipe_string.as_bytes())?;
    let parsed = PackageRecipe::from_file(&tmp_path)
        .map_err(|e| format!("Failed to parse package.cfg: {}", e))?;
    let _ = fs::remove_file(&tmp_path);
    Ok(parsed)
}

// Keep the old function for compatibility with the Debug1 command, but have it use the new helper.
pub fn decompress_tarball(input_file: &str) -> Result<(), Box<dyn std::error::Error>> {
    let input_path = Path::new("/tmp/").join(format!("{}.tar.gz", input_file));
    let dest_dir = Path::new("/tmp/nxpkg_extract");
    extract_tar_gz(&input_path, dest_dir)
}
