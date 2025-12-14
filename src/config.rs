// src/config.rs
// Loads nxpkg configuration from /etc and user config; provides defaults.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub repo_url: String,
    pub db_path: PathBuf,
    pub cache_dir: PathBuf,
    pub require_signed_index: bool,
    pub pubkey_path: PathBuf,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            repo_url: "https://your-server.com/releases".to_string(),
            db_path: PathBuf::from("/var/lib/nxpkg/nxpkg_meta.db"),
            cache_dir: PathBuf::from("/var/cache/nxpkg"),
            require_signed_index: true,
            pubkey_path: PathBuf::from("/etc/nxpkg/nxpkg.pub"),
        }
    }
}

impl AppConfig {
    pub fn load() -> Self {
        let mut cfg = AppConfig::default();

        // 1) Load system config: /etc/nxpkg/config.cfg
        let sys_cfg = Path::new("/etc/nxpkg/config.cfg");
        if sys_cfg.exists() {
            if let Err(e) = Self::apply_cfg_file(&mut cfg, sys_cfg) {
                eprintln!("Warning: failed to load {}: {}", sys_cfg.display(), e);
            }
        }

        // 2) Load user config: $XDG_CONFIG_HOME/nxpkg/config.cfg or ~/.config/nxpkg/config.cfg
        let user_cfg = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("~/.config").expand_home());
        let user_cfg_path = user_cfg.join("nxpkg/config.cfg");
        if user_cfg_path.exists() {
            if let Err(e) = Self::apply_cfg_file(&mut cfg, &user_cfg_path) {
                eprintln!("Warning: failed to load {}: {}", user_cfg_path.display(), e);
            }
        }

        // 3) Environment overrides
        if let Ok(v) = env::var("NXPKG_REPO_URL") { cfg.repo_url = v; }
        if let Ok(v) = env::var("NXPKG_DB_PATH") { cfg.db_path = PathBuf::from(v); }
        if let Ok(v) = env::var("NXPKG_CACHE_DIR") { cfg.cache_dir = PathBuf::from(v); }
        if let Ok(v) = env::var("NXPKG_REQUIRE_SIGNED_INDEX") { cfg.require_signed_index = v == "1" || v.eq_ignore_ascii_case("true"); }
        if let Ok(v) = env::var("NXPKG_PUBKEY_PATH") { cfg.pubkey_path = PathBuf::from(v); }

        // 4) Ensure directories exist for db parent and cache dir
        if let Some(parent) = cfg.db_path.parent() { let _ = fs::create_dir_all(parent); }
        let _ = fs::create_dir_all(&cfg.cache_dir);

        cfg
    }

    fn apply_cfg_file(cfg: &mut AppConfig, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        let mut section = String::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len()-1].trim().to_lowercase();
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                match section.as_str() {
                    "repo" => {
                        if key == "url" { cfg.repo_url = value.to_string(); }
                    }
                    "storage" => {
                        if key == "db_path" { cfg.db_path = PathBuf::from(value); }
                        else if key == "cache_dir" { cfg.cache_dir = PathBuf::from(value); }
                    }
                    "security" => {
                        if key == "require_signed_index" {
                            cfg.require_signed_index = matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes");
                        } else if key == "pubkey_path" {
                            cfg.pubkey_path = PathBuf::from(value);
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

// Small helper to expand leading ~ in paths
trait ExpandHome {
    fn expand_home(self) -> PathBuf;
}

impl ExpandHome for PathBuf {
    fn expand_home(self) -> PathBuf {
        let s = self.to_string_lossy().to_string();
        if let Some(rest) = s.strip_prefix("~/") {
            if let Some(home) = dirs_next::home_dir() {
                return home.join(rest);
            }
        }
        PathBuf::from(s)
    }
}
