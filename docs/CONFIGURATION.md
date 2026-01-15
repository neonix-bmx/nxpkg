# Configuration

nxpkg loads configuration in this order:
1) /etc/nxpkg/config.cfg
2) $XDG_CONFIG_HOME/nxpkg/config.cfg or ~/.config/nxpkg/config.cfg
3) repo remotes files (see below)
4) environment variables (override everything)

## config.cfg
This file uses INI-like sections. All keys are optional.

Example:

```ini
[repo]
url = https://example.com/releases

[storage]
db_path = /var/lib/nxpkg/nxpkg_meta.db
cache_dir = /var/cache/nxpkg

[security]
require_signed_index = true
pubkey_path = /etc/nxpkg/nxpkg.pub
```

## repo_remotes.cfg (binary repos)
Binary repos provide the package index and .nxpkg downloads. You can define multiple remotes and choose an active one. The active remote is used as the repo URL when no explicit URL is set.

Locations:
- /etc/nxpkg/repo_remotes.cfg
- $XDG_CONFIG_HOME/nxpkg/repo_remotes.cfg or ~/.config/nxpkg/repo_remotes.cfg

Example:

```ini
[repo_remotes]
main = https://example.com/releases
testing = https://example.com/testing

[active]
name = main
```

Use `nxpkg repo-remote` to list, add, remove, or select remotes.

## repos.cfg (source repos for buildins)
This list is used by `nxpkg buildins` when searching for source repositories. It prefers configured repos before hitting GitHub or GitLab.

Locations:
- /etc/nxpkg/repos.cfg
- $XDG_CONFIG_HOME/nxpkg/repos.cfg or ~/.config/nxpkg/repos.cfg

Example:

```ini
[repos]
mesa = https://gitlab.freedesktop.org/mesa/mesa.git
linux = https://github.com/torvalds/linux.git
```

Use `nxpkg repos` to list, add, or remove entries.

## Environment variables
- NXPKG_REPO_URL: override repository base URL
- NXPKG_DB_PATH: override SQLite database path
- NXPKG_CACHE_DIR: override cache directory
- NXPKG_REQUIRE_SIGNED_INDEX: set to 1/true to require index signature
- NXPKG_PUBKEY_PATH: public key file path for index verification
- NXPKG_TOKEN: bearer token for publish uploads
- NXPKG_SIGN_KEYPAIR_B64: base64 ed25519 keypair for signing index.json

## Build profiles
Build profiles store build system and extra args by package name in the local DB.
- Use `--save-profile` on `buildins` or `buildpkg` to persist the resolved profile.
- Use `--no-profile` to ignore saved profiles for a build.
