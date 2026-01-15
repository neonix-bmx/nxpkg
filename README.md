# nxpkg

NeoniX package manager and build tooling for Neonix-like distributions. It can install packages from a signed repository index, build packages in a chroot, and package local projects into `.nxpkg` archives.

## Features
- Install, remove, and search packages from a remote repository.
- Signed `index.json` verification with Ed25519.
- Build packages from source in an isolated chroot.
- Build and package local projects to `.nxpkg`.
- Build profiles stored in the local database for repeatable builds.

## Quick start
Build the CLI:

```bash
cargo build --release
```

Basic usage:

```bash
./target/release/nxpkg --help
./target/release/nxpkg search <term>
./target/release/nxpkg install <package>
./target/release/nxpkg remove <package>
```

Build from a remote repository (searches configured repos/GitHub/GitLab):

```bash
sudo ./target/release/nxpkg buildins <repo-term> --package <name> --save-profile
```

Package a local project into `.nxpkg`:

```bash
sudo ./target/release/nxpkg buildpkg --path /path/to/project --package <name> --output-dir /tmp
```

Note: chroot build and package commands require root privileges.

## Commands overview
- `install`: install from repo or local file (`-L`)
- `remove`/`purge`: uninstall packages
- `search`: search repository index
- `buildins`: build from a remote repository in chroot
- `buildpkg`: build a local project and package it
- `repos`: manage configured source repos (`/etc/nxpkg/repos.cfg`, `~/.config/nxpkg/repos.cfg`)
- `repo-remote`: manage binary repo remotes (`/etc/nxpkg/repo_remotes.cfg`, `~/.config/nxpkg/repo_remotes.cfg`)
- `publish`: upload `.nxpkg` and update `index.json`
- `health`: sanity checks (db, cache, repo index, optional chroot tools)

## Documentation
- Build and packaging: `docs/BUILDING.md`
- Configuration: `docs/CONFIGURATION.md`
- Repository format and signing: `docs/REPOSITORY.md`
- Security notes: `docs/SECURITY.md`

## Development
- `cargo check`
- `cargo test` (if/when tests are added)
