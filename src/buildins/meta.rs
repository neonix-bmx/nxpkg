//! src/buildins/meta.rs
//! Handles parsing of package recipe files (.cfg) without external dependencies.

use std::fs;
use std::path::Path;

// --- Data Structures ---
#[derive(Debug, Default, Clone)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub architectures: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct BuildInfo {
    pub dependencies: Vec<String>,
    pub commands: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct InstallInfo {
    pub install_params: Vec<String>,
    // This field is populated at install time, not read from the .cfg
    pub installed_files: Vec<String>, 
}

#[derive(Debug, Default, Clone)]
pub struct PackageRecipe {
    pub package: PackageInfo,
    pub build: BuildInfo,
    pub install: InstallInfo,
}

// --- Zero-Dependency Parser Implementation ---
impl PackageRecipe {
    pub fn from_str(content: &str) -> Result<Self, String> {
        let mut recipe = PackageRecipe::default();
        let mut current_section = "";

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            if line.starts_with('[') && line.ends_with(']') {
                current_section = &line[1..line.len() - 1];
                continue;
            }

            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();

                match current_section {
                    "package" => match key {
                        "name" => recipe.package.name = value.to_string(),
                        "version" => recipe.package.version = value.to_string(),
                        "architectures" => {
                            recipe.package.architectures = value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                        }
                        _ => {}
                    },
                    "build" => match key {
                        "dependencies" => {
                            recipe.build.dependencies = value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                        }
                        "commands" => {
                            recipe.build.commands = value.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                        }
                        _ => {}
                    },
                    "install" => match key {
                        "install_params" => {
                            recipe.install.install_params = value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
        
        if recipe.package.name.is_empty() {
            return Err("Recipe is missing 'name' in [package]".to_string());
        }
        if recipe.package.version.is_empty() {
            return Err("Recipe is missing 'version' in [package]".to_string());
        }

        Ok(recipe)
    }

    pub fn from_file(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Could not read recipe file '{}': {}", path.display(), e))?;
        Self::from_str(&content)
    }
}
