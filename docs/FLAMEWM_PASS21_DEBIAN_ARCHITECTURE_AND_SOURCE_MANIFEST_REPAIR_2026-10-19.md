# FlameWM 0.1.0 — Pass 21 Debian packaging recovery

## Source of truth: user-provided terminal log

- `cargo build --release --workspace`: succeeds (the `stroke_rounded_rect` and `split_label_lines` messages are nonfatal dead-code warnings).
- `./scripts/package` aborts at `package-source-archive` with `ValueError: executable is not registered: scripts/version` after it already emitted a Debian package.
- `dpkg-deb` control metadata advertises `Architecture: any`, although the filename claims `amd64`. `apt` correctly refuses to unpack it.
- `tools/flamewm_deb_acceptance.py` incorrectly marked `Architecture: any` as PASS, then used `dpkg --force-architecture` to make its isolated install/reinstall checks pass.
- `SHA256SUMS` is missing because source packaging aborted before the checksum step. `_apt` permission warning is a separate sandbox accessibility notice on the mounted source directory.
- The patch path in the user's first command did not exist locally; downloading an archive/patch in ChatGPT does not place it into `~/Downloads` automatically.

## Root-level changes

1. Register the executable `scripts/version` in `packaging/source-manifest.txt`. Preserve the strict allowlist: do not disable executable checking to mask this error.
2. Pre-validate complete source manifest before opening an archive. Write it to a temporary file and atomically publish on success, so invalid inputs cannot leave partial `.tar.gz` outputs.
3. Generate Debian binary control with the actual host `amd64`, not source-control placeholder `any`. The current x86_64-only release script refuses non-amd64/native-x86_64 systems instead of mislabeling foreign code.
4. Build and validate the source archive **before** emitting any new `.deb`. Check the generated `.deb` metadata directly.
5. Strengthen Debian acceptance to reject `any`/`all`/missing/mismatched/foreign architectures and require a completed SHA256SUMS. Do not bypass architecture with `--force-architecture`, and do not run the isolated dpkg lifecycle on an invalid architecture.
6. Update installation and packaging docs with exact artifact paths, architecture checks, and `_apt` sandbox workaround via `/tmp`.

## Verified in this authoring environment

- `python3 -I -m unittest discover -s tools -p 'test_package_source_archive.py' -q`: 6 passed.
- `python3 -I -m unittest discover -s tools -p 'test_flamewm_deb_acceptance.py' -q`: 33 passed.
- `python3 -I -m unittest discover -s tools -p 'test_flamewm_package_pipeline.py' -q`: 2 passed, using real `dpkg-deb` and a minimal synthetic release tree (stub binaries, **not** a real FlameWM build).
- End-to-end fake release creates `.target/dist/flamewm_0.1.0_amd64.deb`, writes reproducible source and Linux tarballs and valid `SHA256SUMS`, stages `usr/share/xsessions/flamewm.desktop`, and completes isolated real `dpkg` install / reinstall / status / remove without forcing architecture.
- `bash -n scripts/package`: passes.
- `python3 -I tools/ui_contract_guard.py`: 0 errors and 24 inherited warnings.

## Still unverified / native gates

This environment has neither Cargo nor rustc, and the uploaded source export omits several vendor/icon asset trees, so the **full repository** release packaging and native binary are not tested here. Run on the user's full source checkout:

```bash
CARGO_BUILD_JOBS=2 cargo build --release --workspace
./scripts/package
dpkg-deb -f .target/dist/flamewm_0.1.0_amd64.deb Architecture
(cd .target/dist && sha256sum -c SHA256SUMS)
python3 -I tools/flamewm_deb_acceptance.py --deb .target/dist/flamewm_0.1.0_amd64.deb
```

The Debian field must equal `amd64`; do not install a stale package with `any`. On an x86_64 Debian/Ubuntu target, transfer the newly created `.deb` and run `sudo apt install ./flamewm_0.1.0_amd64.deb` from the directory containing it. The installed X11 SDDM session is `usr/share/xsessions/flamewm.desktop`; no native Wayland session is claimed.
