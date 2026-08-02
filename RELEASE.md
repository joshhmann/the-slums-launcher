# The Slums Launcher — Release Process

## Overview

The launcher builds automatically via GitHub Actions. Pushing to `main` triggers a
Windows + Linux build that is published as a signed "stable" release. The launcher
self-updates via the built-in "Check Launcher Update" button.

## One-time setup (already done, keep this secret safe)

The Tauri updater signing keypair was generated on the build host:

- Private key: `/tmp/tauri-signing-key` (on the LXC build host)
- Public key:  `/tmp/tauri-signing-key.pub` (already embedded in `src-tauri/tauri.conf.json`)

**The private key MUST be added as a GitHub Actions secret** or updates will not
be signed and users cannot install them:

1. GitHub repo → Settings → Secrets and variables → Actions → New repository secret
2. Name: `TAURI_SIGNING_PRIVATE_KEY`
3. Value: the full contents of `/tmp/tauri-signing-key`

Back up this key somewhere safe. If it is lost, users will need to reinstall the
launcher manually (updates will silently fail).

## Releasing

### Normal release (any commit to main)

```bash
git add -A
git commit -m "fix: something"
git push origin main
```

GitHub Actions:
1. Builds Windows (NSIS installer) and Linux (AppImage)
2. Publishes both to the `stable` release (tag `stable`)
3. Generates `latest.json` update manifest (signed)

Users click "Check Launcher Update" → install → launcher restarts on the new build.

### Version bumps

Version lives in `src-tauri/tauri.conf.json` (`"version": "1.0.1"`). Bump it for
meaningful releases:

```bash
# bump version in src-tauri/tauri.conf.json, then:
git add src-tauri/tauri.conf.json
git commit -m "release: v1.0.2"
git push origin main
```

Tag the milestone if you want a named release:

```bash
git tag v1.0.2
git push origin v1.0.2
```

## What the workflow does

`.github/workflows/release.yml`:

- Trigger: push to `main` (or manual `workflow_dispatch`)
- Matrix: `windows-latest` + `ubuntu-latest`
- Steps: checkout → Linux deps → Node 20 → Rust stable → npm ci → tauri build
- `tauri-action` signs with `TAURI_SIGNING_PRIVATE_KEY` and publishes to the
  `stable` release with `latest.json`

## Troubleshooting

| Symptom | Cause / Fix |
|---------|-------------|
| Build fails "signing key not found" | `TAURI_SIGNING_PRIVATE_KEY` secret missing — add it |
| Update check says up to date but new build exists | Old release still `latest`; wait for workflow to finish, or check Actions tab |
| Update downloads but won't install | Key mismatch — pubkey in `tauri.conf.json` doesn't match the private key used at build |
| Need to force a rebuild | Re-run the workflow from Actions tab (workflow_dispatch) |

## Related

- Repo: `https://github.com/joshhmann/the-slums-launcher`
- Webapp: `https://wowslums.asslorde.com` (manifest + addon catalog + downloads)
- Client manifest: `https://wowslums.asslorde.com/api/manifest.json`
- Addon catalog: `https://wowslums.asslorde.com/api/addons.json`
