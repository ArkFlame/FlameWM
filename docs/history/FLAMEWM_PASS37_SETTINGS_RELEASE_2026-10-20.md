# FlameWM 0.1.0 Pass 37 — Complete source release

This release is built from the live complete checkout with the allowlist in
`packaging/source-manifest.txt`. The source archive includes the runtime assets,
licenses, packaging inputs, and `.vendor/RustWebRender-0.0.9` reference tree
present in that checkout. It excludes build output and private/cache data through
the existing `scripts/package-source-archive` rules.

This checkout does not contain the optional icon theme trees
`assets/themes/icons/PlagueSur`, `assets/themes/icons/Fluent`, or
`assets/themes/icons/Fluent-Yellow`. `packaging/install-manifest.txt` labels
PlagueSur and Fluent optional, and the packager copies them only when supplied;
the hard package asset inputs (`assets/cursors/cursors` and `assets/web/breeze`)
are present. FlameWM still defaults to and offers PlagueSur/Fluent, so without
those theme payloads icon lookup continues through Breeze and hicolor. The
source archive preserves the checkout as it stands; this optional visual theme
limitation remains and has no runtime appearance certification here.

Create the distinctive archive after the source and package checks finish:

```bash
mkdir -p .target/dist
python3 scripts/package-source-archive "$PWD" packaging/source-manifest.txt \
  .target/dist/FlameWM_0.1.0_PASS37_COMPLETE_SOURCE_SETTINGS_TEST_FIX_2026-10-20.tar.gz
```

The helper normalizes tar member mtimes to `SOURCE_DATE_EPOCH` (default `0`) for
reproducibility. Do not overlay this archive onto an existing checkout: old
Cargo outputs can outlive replacement source files. Extract into a new directory
and use a new target directory for verification or rebuilding:

```bash
verify_root="$(mktemp -d)"
mkdir "$verify_root/source" "$verify_root/target"
tar --touch -xzf /path/to/FlameWM_0.1.0_PASS37_COMPLETE_SOURCE_SETTINGS_TEST_FIX_2026-10-20.tar.gz \
  -C "$verify_root/source"
cd "$verify_root/source/flamewm-webrender"
CARGO_TARGET_DIR="$verify_root/target" CARGO_BUILD_JOBS=2 cargo test \
  -p flamewm-control-core --test platform_control \
  shell_bootstrap_reconciles_display_and_panels_before_reply
```

`--touch` is GNU tar's `--touch`/`-m` option; GNU tar 1.35 was available when
this guidance was checked. It sets extracted file mtimes to extraction time,
while the new Cargo target directory ensures test binaries are compiled from the
extracted sources. The separate Pass 37 evidence report records archive identity,
checksums, and executed build/test results.
