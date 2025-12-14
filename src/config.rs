// src/config.rs
// Loads nxpkg configuration from /etc and user config; provides defaults.

use std::env;
use std::fs;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub repo_url: String,
    pub db_path: PathBuf,
    pub cache_dir: PathBuf,
    pub require_signed_index: bool,
    pub pubkey_path: PathBuf,
    // Multiple binary repository remotes and active selection
    pub repo_remotes: BTreeMap<String, String>, // name -> url
    pub active_repo: Option<String>,           // name
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            // Leave empty by default; will be resolved from repo_remotes/active or env/config
            repo_url: String::new(),
            db_path: PathBuf::from("/var/lib/nxpkg/nxpkg_meta.db"),
            cache_dir: PathBuf::from("/var/cache/nxpkg"),
            require_signed_index: true,
            pubkey_path: PathBuf::from("/etc/nxpkg/nxpkg.pub"),
            repo_remotes: BTreeMap::new(),
            active_repo: None,
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

        // 2.5) Load repo remotes from files and apply active
        Self::apply_repo_remotes_files(&mut cfg);

        // 3) Environment overrides (highest priority)
        if let Ok(v) = env::var("NXPKG_REPO_URL") { cfg.repo_url = v; }
        if let Ok(v) = env::var("NXPKG_DB_PATH") { cfg.db_path = PathBuf::from(v); }
        if let Ok(v) = env::var("NXPKG_CACHE_DIR") { cfg.cache_dir = PathBuf::from(v); }
        if let Ok(v) = env::var("NXPKG_REQUIRE_SIGNED_INDEX") { cfg.require_signed_index = v == "1" || v.eq_ignore_ascii_case("true"); }
        if let Ok(v) = env::var("NXPKG_PUBKEY_PATH") { cfg.pubkey_path = PathBuf::from(v); }

        // 3.5) Final fallback: if repo_url still empty, try to resolve from remotes
        if cfg.repo_url.trim().is_empty() {
            // Prefer active, else if exactly one remote defined pick that
            let candidate = cfg.active_repo.clone().or_else(|| {
                if cfg.repo_remotes.len() == 1 { cfg.repo_remotes.keys().next().cloned() } else { None }
            });
            if let Some(name) = candidate {
                if let Some(url) = cfg.repo_remotes.get(&name) {
                    cfg.repo_url = url.clone();
                }
            }
        }

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
    fn apply_repo_remotes_files(cfg: &mut AppConfig) {
        // Read system-wide repo remotes
        let sys_file = Path::new("/etc/nxpkg/repo_remotes.cfg");
        if sys_file.exists() {
            if let Err(e) = Self::apply_repo_remotes_from_file(cfg, sys_file) {
                eprintln!("Warning: failed to load {}: {}", sys_file.display(), e);
            }
        }
        // Read user repo remotes
        let user_base = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("~/.config").expand_home());
        let user_file = user_base.join("nxpkg/repo_remotes.cfg");
        if user_file.exists() {
            if let Err(e) = Self::apply_repo_remotes_from_file(cfg, &user_file) {
                eprintln!("Warning: failed to load {}: {}", user_file.display(), e);
            }
        }

        // Apply active selection to repo_url if present
        if let Some(active) = cfg.active_repo.clone() {
            if let Some(url) = cfg.repo_remotes.get(&active) {
                cfg.repo_url = url.clone();
            }
        }
    }

    fn apply_repo_remotes_from_file(cfg: &mut AppConfig, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        let mut section = String::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') { continue; }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len()-1].trim().to_lowercase();
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                match section.as_str() {
                    "repo_remotes" => { cfg.repo_remotes.insert(key.to_string(), value.to_string()); }
                    "active" => {
                        if key.eq_ignore_ascii_case("name") || key.eq_ignore_ascii_case("active") {
                            cfg.active_repo = Some(value.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    // User-facing helpers to manage repo_remotes in user config file
    pub fn user_repo_remotes_path() -> PathBuf {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("~/.config").expand_home())
            .join("nxpkg/repo_remotes.cfg")
    }

    pub fn save_repo_remotes(map: &BTreeMap<String,String>, active: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::user_repo_remotes_path();
        if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
        let mut out = String::new();
        out.push_str("[repo_remotes]\n");
        for (k, v) in map { out.push_str(&format!("{} = {}\n", k, v)); }
        if let Some(a) = active { out.push_str("\n[active]\n"); out.push_str(&format!("name = {}\n", a)); }
        fs::write(path, out)?;
        Ok(())
    }

    pub fn add_repo_remote(name: &str, url: &str) -> Result<(), Box<dyn std::error::Error>> {
        // Load current user mapping
        let mut map: BTreeMap<String, String> = BTreeMap::new();
        // Merge system and user for context (we only write user)
        let mut tmp = AppConfig::default();
        Self::apply_repo_remotes_files(&mut tmp);
        map.extend(tmp.repo_remotes);
        map.insert(name.trim().to_string(), url.trim().to_string());
        let active = tmp.active_repo.as_deref();
        Self::save_repo_remotes(&map, active)
    }

    pub fn remove_repo_remote(name: &str) -> Result<(), Box<dyn std::error::Error>> {
        let mut tmp = AppConfig::default();
        Self::apply_repo_remotes_files(&mut tmp);
        let mut map = tmp.repo_remotes;
        let was_active = tmp.active_repo.clone();
        map.remove(name);
        let new_active = match was_active.as_deref() { Some(n) if n == name => None, other => other.map(|s| s.to_string()) };
        Self::save_repo_remotes(&map, new_active.as_deref())
    }

    pub fn set_active_repo(name: &str) -> Result<(), Box<dyn std::error::Error>> {
        let mut tmp = AppConfig::default();
        Self::apply_repo_remotes_files(&mut tmp);
        if !tmp.repo_remotes.contains_key(name) {
            return Err(format!("repo remote '{}' not found", name).into());
        }
        Self::save_repo_remotes(&tmp.repo_remotes, Some(name))
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
