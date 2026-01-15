use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use tar::{Archive, Builder, EntryType};
use tempfile::{NamedTempFile, TempDir};
use walkdir::WalkDir;
use crate::buildins::meta::PackageRecipe; // Import the recipe struct

#[cfg(unix)]
use std::os::unix::fs::{PermissionsExt, symlink};

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
    let _ = unpack_archive_safe(&mut archive, dest_dir)?;

    Ok(())
}

/// Extracts a .nxpkg, parses its recipe, and installs files to their final destinations.
///
/// Returns a tuple containing:
/// 1. The parsed `PackageRecipe`.
/// 2. A `Vec<PathBuf>` of the absolute paths of the installed files.
pub fn extract_nxpkg(nxpkg_path: &Path) -> Result<(PackageRecipe, Vec<PathBuf>), Box<dyn std::error::Error>> {
    let mut archive = open_nxpkg_archive(nxpkg_path)?;
    let mut recipe_text: Option<String> = None;
    let mut data_file: Option<NamedTempFile> = None;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_type = entry.header().entry_type();
        if !matches!(entry_type, EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse) {
            continue;
        }

        let entry_path = entry.path()?;
        let rel = sanitize_entry_path(&entry_path)?;
        if rel == Path::new("package.cfg") {
            let mut buf = String::new();
            entry.read_to_string(&mut buf)?;
            recipe_text = Some(buf);
        } else if rel == Path::new("data.tar.gz") {
            let mut tmp = NamedTempFile::new()?;
            std::io::copy(&mut entry, &mut tmp)?;
            tmp.flush()?;
            data_file = Some(tmp);
        }
    }

    let recipe_text = recipe_text.ok_or("Invalid .nxpkg: 'package.cfg' not found.")?;
    let recipe = PackageRecipe::from_str(&recipe_text)
        .map_err(|e| format!("Failed to parse package.cfg: {}", e))?;

    // Architecture validation BEFORE installing anything.
    let supports_current_arch = {
        fn norm(s: &str) -> String { s.trim().to_lowercase().replace('-', "_") }
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
            aliases.extend(["any", "noarch"].iter().copied());
            let aliases: Vec<String> = aliases.into_iter().map(|s| s.to_string()).collect();
            declared.iter().any(|d| aliases.iter().any(|a| a == d))
        }
    };

    if !supports_current_arch {
        return Err(format!(
            "Package is not built for this architecture (host: {}, package: {:?})",
            std::env::consts::ARCH,
            recipe.package.architectures
        ).into());
    }

    let data_file = data_file.ok_or("Invalid .nxpkg: 'data.tar.gz' not found.")?;
    let file = File::open(data_file.path())?;
    let reader = BufReader::new(file);
    let decompressor = GzDecoder::new(reader);
    let mut archive = Archive::new(decompressor);
    let installed_files = unpack_archive_safe(&mut archive, Path::new("/"))?;

    Ok((recipe, installed_files))
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
    let tmp_dir = TempDir::new().map_err(|e| e.to_string())?;
    let data_tar_gz_path = tmp_dir.path().join("data.tar.gz");
    {
        let data_file = File::create(&data_tar_gz_path).map_err(|e| e.to_string())?;
        let enc = GzEncoder::new(data_file, Compression::default());
        let mut tar_builder = Builder::new(enc);

        // Add directories and files preserving relative paths
        for entry in WalkDir::new(staging_dir).follow_links(false).into_iter().filter_map(Result::ok) {
            let rel = entry.path().strip_prefix(staging_dir).map_err(|e| e.to_string())?;
            if rel.as_os_str().is_empty() {
                continue;
            }
            if entry.file_type().is_dir() {
                tar_builder.append_dir(rel, entry.path()).map_err(|e| e.to_string())?;
            } else if entry.file_type().is_file() {
                tar_builder.append_path_with_name(entry.path(), rel).map_err(|e| e.to_string())?;
            } else if entry.file_type().is_symlink() {
                let target = fs::read_link(entry.path()).map_err(|e| e.to_string())?;
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(EntryType::Symlink);
                header.set_size(0);
                header.set_mode(0o777);
                #[cfg(unix)]
                if let Ok(meta) = fs::symlink_metadata(entry.path()) {
                    header.set_mode(meta.permissions().mode());
                }
                header.set_link_name(&target).map_err(|e| e.to_string())?;
                header.set_cksum();
                tar_builder.append_data(&mut header, rel, std::io::empty()).map_err(|e| e.to_string())?;
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
    Ok(())
}

/// Read only the package.cfg (recipe) from a .nxpkg without installing anything.
/// Supports both plain tar and gzipped outer container.
pub fn read_recipe_from_nxpkg(nxpkg_path: &Path) -> Result<PackageRecipe, Box<dyn std::error::Error>> {
    let mut archive = open_nxpkg_archive(nxpkg_path)?;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_type = entry.header().entry_type();
        if !matches!(entry_type, EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse) {
            continue;
        }
        let entry_path = entry.path()?;
        let rel = sanitize_entry_path(&entry_path)?;
        if rel == Path::new("package.cfg") {
            let mut recipe_content = String::new();
            entry.read_to_string(&mut recipe_content)?;
            return PackageRecipe::from_str(&recipe_content)
                .map_err(|e| format!("Failed to parse package.cfg: {}", e).into());
        }
    }
    Err("package.cfg not found in .nxpkg".into())
}

// Keep the old function for compatibility with the Debug1 command, but have it use the new helper.
pub fn decompress_tarball(input_file: &str) -> Result<(), Box<dyn std::error::Error>> {
    let input_path = Path::new("/tmp/").join(format!("{}.tar.gz", input_file));
    let dest_dir = Path::new("/tmp/nxpkg_extract");
    extract_tar_gz(&input_path, dest_dir)
}

fn open_nxpkg_archive(nxpkg_path: &Path) -> Result<Archive<Box<dyn Read>>, Box<dyn std::error::Error>> {
    let file = File::open(nxpkg_path)?;
    let mut reader = BufReader::new(file);
    let mut magic = [0u8; 2];
    let _ = reader.read(&mut magic)?;
    reader.seek(SeekFrom::Start(0))?;

    let boxed: Box<dyn Read> = if magic == [0x1f, 0x8b] {
        Box::new(GzDecoder::new(reader))
    } else {
        Box::new(reader)
    };

    Ok(Archive::new(boxed))
}

fn sanitize_entry_path(path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut clean = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(p) => clean.push(p),
            Component::CurDir => {}
            _ => {
                return Err(format!("Invalid entry path in archive: {}", path.display()).into());
            }
        }
    }
    if clean.as_os_str().is_empty() {
        return Err("Invalid empty entry path in archive".into());
    }
    Ok(clean)
}

fn validate_link_target(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for comp in path.components() {
        match comp {
            Component::ParentDir | Component::Prefix(_) => {
                return Err(format!("Invalid symlink target: {}", path.display()).into());
            }
            _ => {}
        }
    }
    Ok(())
}

fn ensure_no_symlink_parents(
    dest_root: &Path,
    dest_path: &Path,
    created_symlinks: &HashSet<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let parent = dest_path.parent().ok_or("Invalid entry path")?;
    let rel_parent = parent.strip_prefix(dest_root)
        .map_err(|_| format!("Entry escapes destination root: {}", dest_path.display()))?;

    let mut current = dest_root.to_path_buf();
    for comp in rel_parent.components() {
        current.push(comp);
        if created_symlinks.contains(&current) {
            return Err(format!("Refusing to traverse symlink created by archive: {}", current.display()).into());
        }
        if dest_root != Path::new("/") {
            if let Ok(meta) = fs::symlink_metadata(&current) {
                if meta.file_type().is_symlink() {
                    return Err(format!("Refusing to traverse symlinked parent: {}", current.display()).into());
                }
            }
        }
    }
    Ok(())
}

fn unpack_archive_safe<R: Read>(archive: &mut Archive<R>, dest_root: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut installed = Vec::new();
    let mut created_symlinks: HashSet<PathBuf> = HashSet::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_type = entry.header().entry_type();

        match entry_type {
            EntryType::XHeader | EntryType::XGlobalHeader | EntryType::GNULongName | EntryType::GNULongLink => {
                continue;
            }
            _ => {}
        }

        let entry_path = entry.path()?;
        let rel = sanitize_entry_path(&entry_path)?;
        let dest_path = dest_root.join(&rel);

        ensure_no_symlink_parents(dest_root, &dest_path, &created_symlinks)?;

        match entry_type {
            EntryType::Directory => {
                if let Ok(meta) = fs::symlink_metadata(&dest_path) {
                    if meta.file_type().is_symlink() {
                        return Err(format!("Refusing to create directory over symlink: {}", dest_path.display()).into());
                    }
                }
                let existed = dest_path.exists();
                fs::create_dir_all(&dest_path)?;
                #[cfg(unix)]
                if !existed {
                    if let Ok(mode) = entry.header().mode() {
                        fs::set_permissions(&dest_path, fs::Permissions::from_mode(mode & 0o777))?;
                    }
                }
            }
            EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse => {
                if let Some(parent) = dest_path.parent() {
                    fs::create_dir_all(parent)?;
                }

                if let Ok(meta) = fs::symlink_metadata(&dest_path) {
                    if meta.file_type().is_dir() {
                        return Err(format!("Refusing to overwrite directory with file: {}", dest_path.display()).into());
                    }
                    let _ = fs::remove_file(&dest_path);
                }

                let mut out = OpenOptions::new().create(true).truncate(true).write(true).open(&dest_path)?;
                std::io::copy(&mut entry, &mut out)?;
                #[cfg(unix)]
                if let Ok(mode) = entry.header().mode() {
                    fs::set_permissions(&dest_path, fs::Permissions::from_mode(mode & 0o777))?;
                }
                installed.push(dest_path);
            }
            EntryType::Symlink => {
                let link_target = entry.link_name()?
                    .ok_or("Symlink entry missing link target")?;
                validate_link_target(&link_target)?;

                if let Some(parent) = dest_path.parent() {
                    fs::create_dir_all(parent)?;
                }

                if let Ok(meta) = fs::symlink_metadata(&dest_path) {
                    if meta.file_type().is_dir() {
                        return Err(format!("Refusing to overwrite directory with symlink: {}", dest_path.display()).into());
                    }
                    let _ = fs::remove_file(&dest_path);
                }

                #[cfg(unix)]
                symlink(&link_target, &dest_path)?;
                #[cfg(not(unix))]
                return Err("Symlink entries are not supported on this platform".into());

                created_symlinks.insert(dest_path.clone());
                installed.push(dest_path);
            }
            EntryType::Link => {
                return Err("Hard link entries are not supported for security reasons".into());
            }
            EntryType::Char | EntryType::Block | EntryType::Fifo => {
                return Err("Special device entries are not supported".into());
            }
            _ => {
                return Err("Unsupported archive entry type".into());
            }
        }
    }

    Ok(installed)
}
