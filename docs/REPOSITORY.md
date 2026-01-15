# Repository Format and Publishing

A repository is a base URL that serves these files:
- index.json (package catalog)
- index.json.sig (optional signature, base64)
- package archives (.nxpkg)

The `nxpkg` client downloads `index.json`, optionally verifies its signature, then downloads the selected .nxpkg file.

## index.json format
The file is a JSON object with a `packages` map. Each entry describes the latest version and download location.

Example:

```json
{
  "packages": {
    "hello": {
      "latest_version": "1.2.3",
      "description": "Example package",
      "download_url": "https://example.com/releases/hello-1.2.3.nxpkg",
      "sha256": "<sha256 hex>",
      "architectures": {
        "x86_64": {
          "download_url": "https://example.com/releases/hello-1.2.3.nxpkg",
          "sha256": "<sha256 hex>"
        },
        "aarch64": {
          "download_url": "https://example.com/releases/hello-1.2.3-aarch64.nxpkg",
          "sha256": "<sha256 hex>"
        },
        "any": {
          "download_url": "https://example.com/releases/hello-1.2.3-any.nxpkg",
          "sha256": "<sha256 hex>"
        }
      }
    }
  }
}
```

Notes:
- `architectures` is optional. If present, it is preferred.
- `download_url` and `sha256` at the top level are legacy fields used as a fallback.
- Architecture keys are matched case-insensitively and support aliases such as x64/amd64, arm64, armv7, i386, and the special tokens `any` and `noarch`.

## Publishing packages
Use the `publish` command to upload a .nxpkg and update index.json:

```bash
nxpkg publish /path/to/pkg.nxpkg --repo https://example.com/releases
```

Behavior:
- Uploads the package to `repo_url/<name>-<version>.nxpkg` via HTTP PUT.
- Updates or creates `index.json` and uploads it via HTTP PUT.
- Computes SHA-256 and stores it in the index.
- Optionally signs the index and uploads `index.json.sig`.

Auth and signing:
- Use `--token` or `NXPKG_TOKEN` for bearer auth.
- Use `--sign-keypair-b64` / `--sign-keypair-file` or `NXPKG_SIGN_KEYPAIR_B64`.
- The keypair is base64 and must decode to 64 bytes (ed25519 private+public).

Your repository endpoint must accept HTTP PUT for `index.json`, `index.json.sig`, and package files.
