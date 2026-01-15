# Security Notes

This document summarizes the security posture of nxpkg and the expectations for safe use.

## Repository integrity
- The repository index can be signed with ed25519 and verified by the client.
- Signature verification uses `index.json` and `index.json.sig` (base64).
- The public key is read from `pubkey_path` (default: /etc/nxpkg/nxpkg.pub) and must be base64.
- If `require_signed_index` is enabled (default), index downloads fail when a valid signature is missing.

## Package integrity
- Package downloads are verified against SHA-256 if the index entry includes a checksum.
- If a checksum is missing, the download is not verified.

## Safe extraction
- .nxpkg archives are extracted with path sanitization to prevent directory traversal.
- Symlink entries are supported but validated; targets cannot contain `..` or absolute prefixes.
- Extraction refuses archive-created symlink traversal and rejects hard links and special device entries.

## Build isolation
- `buildins` and `buildpkg` run in a chroot with new mount, PID, and UTS namespaces.
- `/proc` is mounted with nosuid/noexec/nodev; `/dev` is remounted nosuid/noexec; `/sys` is remounted read-only.
- The build process drops to the `nobody` user inside the chroot.

## Limitations
- Chroot is not a full sandbox. It still relies on the host kernel and shares the host network.
- Builds require root to set up the chroot. Treat build inputs as untrusted and prefer a VM or container if you need stronger isolation.
