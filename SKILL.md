# AeorDB Client Release Deploy Steps

This is the release runbook for rebuilding AeorDB Client binaries, staging them in `../aeordb-www/downloads`, signing the update manifest, and deploying the signed version metadata.

## Guardrails

- Do not use Docker for cross-platform builds.
- Build Linux locally from this repo.
- Build macOS over SSH on `wyatt-mac`.
- Build Windows over SSH on `win11vm`.
- Keep client artifacts named with the `aeordb-client-*` prefix. Do not confuse them with engine artifacts named `aeordb-*`.
- Never paste or commit the private signing key contents. Passing the key path to the signing script is expected.
- Check `git status --short` before editing, building, copying artifacts, committing, or deploying.
- Do not broad-kill every process containing `aeordb-client`; use targeted process management.
- `Cargo.lock` is part of the release snapshot. Remote builds should use `--locked` so all platform binaries use the same dependency resolution.

## Paths

From the normal workspace layout:

```text
aeordb-client/                 this repo
aeordb-www/downloads/          release artifact destination
aeor-signing-tools/            update manifest signing tools
```

Important paths:

```text
Client repo:          ~/Projects/aeordb-workspace/aeordb-client
Website repo:         ~/Projects/aeordb-workspace/aeordb-www
Downloads directory:  ~/Projects/aeordb-workspace/aeordb-www/downloads
Manifest script:      ~/Projects/aeordb-workspace/aeordb-client/scripts/emit-manifest.sh
Signer binary:        ~/Projects/aeor-signing-tools/target/release/aeor-sign-update-manifest
Signing key:          ~/Documents/work/AEOR Development/private/aeor-202605132323-private-key.bin
```

Release artifact names expected by `scripts/emit-manifest.sh`:

```text
aeordb-client-linux-x86_64
aeordb-client-windows-x86_64.exe
aeordb-client-macos
manifest.json
manifest.sig.json
```

The macOS artifact is a universal binary. The manifest maps both `macos-aarch64` and `macos-x86_64` to `aeordb-client-macos`.

## 1. Bump Version

Bump the version in both files:

```text
aeordb-client/Cargo.toml
aeordb-client/tauri.conf.json
```

Run a build or check afterward so `Cargo.lock` reflects the new package version if Cargo changes it. The self-update prompt only appears when the manifest version is newer than the running client's `CARGO_PKG_VERSION`.

## 2. Build Linux

From this repo:

```bash
cd ~/Projects/aeordb-workspace/aeordb-client
./scripts/build.sh --release --bin aeordb-client
cp target/release/aeordb-client ../aeordb-www/downloads/aeordb-client-linux-x86_64
chmod 0644 ../aeordb-www/downloads/aeordb-client-linux-x86_64
```

`scripts/build.sh` defaults to `-j 2` to avoid OOM and system disruption. Override only when there is a concrete reason:

```bash
AEORDB_BUILD_JOBS=4 ./scripts/build.sh --release --bin aeordb-client
```

## 3. Build macOS

Build on `wyatt-mac`, then copy the universal binary back:

```bash
ssh wyatt-mac '
  set -e
  cd ~/Projects/aeordb-workspace/aeordb-client
  git fetch origin
  git checkout main
  git pull --ff-only
  cargo build -j 2 --locked --release --target aarch64-apple-darwin --bin aeordb-client
  cargo build -j 2 --locked --release --target x86_64-apple-darwin --bin aeordb-client
  lipo -create \
    target/aarch64-apple-darwin/release/aeordb-client \
    target/x86_64-apple-darwin/release/aeordb-client \
    -output target/release/aeordb-client-macos
'

scp 'wyatt-mac:~/Projects/aeordb-workspace/aeordb-client/target/release/aeordb-client-macos' \
  ~/Projects/aeordb-workspace/aeordb-www/downloads/aeordb-client-macos
chmod 0644 ~/Projects/aeordb-workspace/aeordb-www/downloads/aeordb-client-macos
```

If the remote branch has local changes, stop and inspect instead of forcing it clean.

## 4. Build Windows

The Windows checkout is currently at `C:\Users\wyatt\Projects\aeordb-client`. Build on `win11vm` using the Visual Studio MSVC environment, then copy the `.exe` back:

```bash
ssh win11vm 'cmd /c "call \"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat\" && cd /d C:\Users\wyatt\Projects\aeordb-client && git fetch origin && git checkout main && git pull --ff-only && cargo build -j 2 --locked --release --bin aeordb-client"'

scp 'win11vm:C:/Users/wyatt/Projects/aeordb-client/target/release/aeordb-client.exe' \
  ~/Projects/aeordb-workspace/aeordb-www/downloads/aeordb-client-windows-x86_64.exe
chmod 0644 ~/Projects/aeordb-workspace/aeordb-www/downloads/aeordb-client-windows-x86_64.exe
```

If the `aeordb` path dependency is missing or broken on Windows, it may need to be restored as a junction to the engine checkout. Inspect before changing it; earlier notes mention `mklink /J` for this host.

## 5. Emit And Sign Manifest

Build the signer if needed:

```bash
cd ~/Projects/aeor-signing-tools
cargo build --release --bin aeor-sign-update-manifest
```

Emit and sign the update manifest:

```bash
cd ~/Projects/aeordb-workspace/aeordb-client
scripts/emit-manifest.sh \
  --key "$HOME/Documents/work/AEOR Development/private/aeor-202605132323-private-key.bin"
```

This writes:

```text
../aeordb-www/downloads/manifest.json
../aeordb-www/downloads/manifest.sig.json
```

The script normalizes artifact permissions to `0644`, computes sizes and SHA-256 hashes, sets `kind` to `aeordb-update-manifest`, and signs the JCS-canonical manifest bytes.

## 6. Verify Before Deploy

```bash
cd ~/Projects/aeordb-workspace/aeordb-www
ls -lh downloads/aeordb-client-linux-x86_64 \
       downloads/aeordb-client-windows-x86_64.exe \
       downloads/aeordb-client-macos \
       downloads/manifest.json \
       downloads/manifest.sig.json

sed -n '1,120p' downloads/manifest.json
sed -n '1,80p' downloads/manifest.sig.json
git status --short
```

Confirm:

- The manifest version is the intended release version.
- The platform filenames are the `aeordb-client-*` artifacts.
- The manifest has both `macos-aarch64` and `macos-x86_64` mapped to `aeordb-client-macos`.
- The website repo status only contains intended release artifacts and manifest changes.

## 7. Deploy

Deploy signed version metadata:

```bash
cd ~/Projects/aeordb-workspace/aeordb-www
./deploy.sh --with-versions
```

`--with-versions` rsyncs the website/downloads to `FS-Server1:/mnt/web/www/aeordb/`, then runs:

```text
aeordb-www-server update-versions /mnt/web/www/aeordb/downloads/manifest.json
```

It prompts for `AW_DB_ROOT_KEY` if the environment variable is not already set.

Use `./deploy.sh --with-server --with-versions` only when the `aeordb-www` server binary itself also needs to be rebuilt and deployed.

## 8. Verify After Deploy

```bash
curl -fsS https://aeordb.com/api/version | head -c 1000
curl -I https://aeordb.com/downloads/aeordb-client-linux-x86_64
curl -I https://aeordb.com/downloads/aeordb-client-windows-x86_64.exe
curl -I https://aeordb.com/downloads/aeordb-client-macos
```

## 9. Commit And Push

Usually two repos are involved:

- `aeordb-client`: version bump and any release-related client code changes.
- `aeordb-www`: binaries in `downloads/`, `manifest.json`, and `manifest.sig.json`.

Commit only the intended files. Do not commit local session artifacts such as `.codex/`, `.playwright-mcp/`, screenshots, logs, or ad hoc notes unless explicitly requested.
