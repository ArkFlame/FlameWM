# FlameWM 0.1.0 — Pass 22: deterministic Debian release recovery

## Failure being repaired

The observed terminal sequence ran:

```bash
PATCH="$HOME/Downloads/FlameWM_0.1.0_pass20_to_pass21_debian_package_repair.patch"
test -f "$PATCH" && git apply --check "$PATCH" && git apply "$PATCH" && CARGO_BUILD_JOBS=2 cargo build --release --workspace && ./scripts/package
```

The patch was absent from the specified path. `test -f` returned nonzero, so **none** of the later commands executed. The user then inspected an *older* `.target/dist/flamewm_0.1.0_amd64.deb` with `Architecture: any`, and `SHA256SUMS` was absent. The real Debian installer correctly rejected this artifact. The Pass 21 `scripts/package` already uses `@ARCH@` substitution and registers executable `scripts/version`; reapplying that source change without executing the packaging command would not address the observed failure.

## New one-command workflow

Overlay Pass 22 source files onto the **complete local checkout**, retaining its `assets/` directory, then run:

```bash
./scripts/build-deb
```

`build-deb` refuses missing tools/assets or foreign host architecture, **removes the old Debian artifact and checksum before Cargo executes**, builds the current workspace, runs `scripts/package`, validates the actual generated `Architecture: amd64`, verifies SHA256SUMS, and checks that `usr/share/xsessions/flamewm.desktop` exists in the `.deb`. Only a fully passing build prints `FLAMEWM-DEB-READY`. Install the exact path printed there. It is not necessary to download or apply any separate `.patch`.

The normal `scripts/package` was also changed to validate/build the source archive *before* doing large install staging. An `EXIT` trap deletes incomplete release outputs whenever packaging fails. It must never leave a nominally fresh `.deb` without checksums after a failed build.

## What is and is not verified

- Tested real `dpkg-deb` and `dpkg` tools in isolated fixtures using nine executable stubs, not native FlameWM binaries.
- Test verifies correct `amd64` metadata, SHA256SUMS, X11 session desktop file and failure of old-artifact publication when Cargo returns nonzero.
- The upstream Python source-archive and Debian acceptance suites still pass.
- The supplied source transfer archive is intentionally compact: it omits the large active asset tree. **It is not standalone buildable**; apply its contents to the existing complete FlameWM checkout.
- The full real `cargo build --release --workspace` and SDDM login have not been run in the authoring environment. Run `./scripts/build-deb` on your machine, and send its complete error output if anything fails.

## Install

```bash
./scripts/build-deb
sudo apt install "$(pwd)/.target/dist/flamewm_0.1.0_amd64.deb"
```

Do not run the `sudo` step unless the first step prints `FLAMEWM-DEB-READY`. FlameWM installs `/usr/share/xsessions/flamewm.desktop`, allowing SDDM to discover the X11 session. It does not install SDDM or provide Wayland compositor support.
