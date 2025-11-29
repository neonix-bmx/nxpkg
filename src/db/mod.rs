use crate::buildins::meta::{BuildInfo, InstallInfo, PackageInfo, PackageRecipe};
use rusqlite::{Connection, Result};
pub mod download;

pub struct PackageManagerDB {
    pub db: Connection,
}

impl PackageManagerDB {
    pub fn new(path: &str) -> Result<Self> {
        let db = Connection::open(path)?;
        Self::init_database(&db)?;
        Ok(PackageManagerDB { db })
    }

    pub fn init_database(db: &Connection) -> Result<()> {
        db.execute(
            "CREATE TABLE IF NOT EXISTS packages (
                name TEXT PRIMARY KEY,
                version TEXT NOT NULL,
                architectures TEXT,
                dependencies TEXT,
                build_commands TEXT,
                install_params TEXT,
                installed_files TEXT
            )",
            [],
        )?;
        Ok(())
    }

    pub fn save_package_metadata(&self, recipe: &PackageRecipe) -> Result<()> {
        let architectures = recipe.package.architectures.join(",");
        let dependencies = recipe.build.dependencies.join(",");
        let build_commands = recipe.build.commands.join(";");
        let install_params = recipe.install.install_params.join(",");
        let installed_files = recipe.install.installed_files.join(";");

        self.db.execute(
            "INSERT OR REPLACE INTO packages (name, version, architectures, dependencies, build_commands, install_params, installed_files)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            &[
                &recipe.package.name,
                &recipe.package.version,
                &architectures,
                &dependencies,
                &build_commands,
                &install_params,
                &installed_files,
            ],
        )?;
        Ok(())
    }

    pub fn get_package_metadata(&self, name: &str) -> Result<Option<PackageRecipe>> {
        let mut stmt = self.db.prepare("SELECT version, architectures, dependencies, build_commands, install_params, installed_files FROM packages WHERE name = ?1")?;
        
        let recipe_result = stmt.query_row([name], |row| {
            let architectures_str: String = row.get(1)?;
            let dependencies_str: String = row.get(2)?;
            let build_commands_str: String = row.get(3)?;
            let install_params_str: String = row.get(4)?;
            let installed_files_str: String = row.get(5).unwrap_or_default(); // Safely handle old entries
            
            Ok(PackageRecipe {
                package: PackageInfo {
                    name: name.to_string(),
                    version: row.get(0)?,
                    architectures: architectures_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
                },
                build: BuildInfo {
                    dependencies: dependencies_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
                    commands: build_commands_str.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
                },
                install: InstallInfo {
                    install_params: install_params_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
                    installed_files: installed_files_str.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
                }
            })
        });

        match recipe_result {
            Ok(recipe) => Ok(Some(recipe)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn rem_package_metadata(&self, name: &str) -> Result<()> {
        // First, retrieve the metadata to know which files to delete.
        if let Some(recipe) = self.get_package_metadata(name)? {
            // Iterate over the stored file paths and delete each one.
            for file_path_str in &recipe.install.installed_files {
                let file_path = std::path::Path::new(file_path_str);
                if file_path.exists() {
                    if let Err(e) = std::fs::remove_file(file_path) {
                        // Log or handle the error, e.g., by collecting failures.
                        // For now, we print to stderr. A more robust solution might be needed.
                        eprintln!("Warning: could not remove file {}: {}", file_path.display(), e);
                    }
                }
            }
            
            // After deleting files, try to remove now-empty parent directories.
            // This is a simple approach. A more robust implementation would track directories
            // created by the package manager and only remove those.
            let mut dirs_to_check: std::collections::HashSet<_> = recipe.install.installed_files
                .iter()
                .filter_map(|p| std::path::Path::new(p).parent())
                .map(|p| p.to_path_buf())
                .collect();
            
            // Sort by path depth (longest first) to remove child directories before parents.
            let mut sorted_dirs: Vec<_> = dirs_to_check.into_iter().collect();
            sorted_dirs.sort_by_key(|b| std::cmp::Reverse(b.as_os_str().len()));

            for dir in sorted_dirs {
                if dir.is_dir() && dir.read_dir().map_or(false, |mut i| i.next().is_none()) {
                    if let Err(e) = std::fs::remove_dir(&dir) {
                        eprintln!("Warning: could not remove directory {}: {}", dir.display(), e);
                    }
                }
            }
        }
        
        // Finally, remove the package entry from the database.
        self.db.execute("DELETE FROM packages WHERE name = ?", [name])?;
        Ok(())
    }
}