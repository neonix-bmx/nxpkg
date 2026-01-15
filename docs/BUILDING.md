# Building and Packaging

This document covers building the `nxpkg` CLI and producing `.nxpkg` packages.

## Build prerequisites
- Rust toolchain (stable)
- pkg-config
- OpenSSL development headers
- SQLite3 development headers

Examples:
- Debian/Ubuntu: `apt install pkg-config libssl-dev libsqlite3-dev`
- Fedora: `dnf install pkgconf-pkg-config openssl-devel sqlite-devel`
- Alpine: `apk add pkgconf openssl-dev sqlite-dev`

## Build the CLI

```bash
cargo build --release
```

If `openssl-sys` fails to build, make sure `pkg-config` and the OpenSSL dev package are installed.

## Build packages from remote sources (buildins)
This flow searches configured repos first, then GitHub/GitLab, clones the repo, builds it in a chroot, installs into a staging directory, and packages the result.

```bash
sudo nxpkg buildins <repo-term> --package <name> --output-dir /tmp
```

Useful options:
- `--build-system {cargo|meson|cmake|scons|make}`
- `--configure-arg <arg>` (repeatable)
- `--build-arg <arg>` (repeatable)
- `--install-arg <arg>` (repeatable)
- `--staging-dir /pkg` (default)
- `--save-profile`
- `--no-profile`

## Build packages from local projects (buildpkg)
Use this when you already have the source on disk.

```bash
sudo nxpkg buildpkg --path /path/to/project --package <name> --output-dir /tmp
```

Options are the same as `buildins` for build system and args.

## Chroot requirements
Chroot execution requires root. The build environment copies needed tools into the chroot. Ensure these are in PATH on the host:

- bash, sh, env
- make, gcc, g++
- cargo, meson, ninja, cmake
- git, scons, python, ld

If a tool is missing, the build will warn and may fail depending on the project.
