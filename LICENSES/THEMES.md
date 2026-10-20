# Bundled icon and cursor themes

Source packs live in `assets/theme-sources/` (local provenance only: git-ignored, excluded from the source archive and from every installed package; `assets/theme-sources/SHA256SUMS` is tracked). Only the normalized runtime payloads under `assets/themes/` are distributed.
The product now offers PlagueSur and Fluent only. Historical pack receipts are archival; the removed icon packs are no longer packaged by `scripts/package`. Fluent source assets are not present in the trimmed export and require a separately licensed installation.

| Pack | Installed as | Upstream | License | Payload evidence | Source archive sha256 | Bundled |
|---|---|---|---|---|---|---|
| 01-McMojave-circle.tar.xz | `assets/themes/icons/McMojave-circle` (light only; `-dark` and `-light` variants dropped) | https://github.com/vinceliuice/McMojave-circle | GPL-3.0 | `COPYING` (GPL v3 text, copied to `LICENSES/McMojave-circle-GPL-3.0.txt`) and `AUTHORS` present in payload | 3c4e4ed0d7b6ceb5377bc8a8b89493b0071665001bf32dd6e8b43bed09e5a741 | legacy, no longer installed |
| McMojave-cursors.tar.xz | `assets/themes/cursors/McMojave-cursors` | https://github.com/vinceliuice/McMojave-cursors | UPSTREAM-DECLARED/UNVERIFIED-IN-PAYLOAD (GPL-3.0 per upstream; no license file in payload) | none in payload | 1f5796833bd5c285be970a196afc7fb76e7aec15135acd75a28043ef6d948f94 | legacy, no longer installed |
| candy-icons.tar.xz | `assets/themes/icons/candy-icons` (`preview/` screenshots and one absolute symlink to `/usr/share/icons/breeze-dark/places/16/folder-library.svg` dropped) | https://github.com/EliverLara/candy-icons | GPL-3.0 | `LICENSE` (GPL v3 text, copied to `LICENSES/candy-icons-GPL-3.0.txt`) in payload; `index.theme` unmodified | 290d2cab67a9f703e8e87aadde3e478d63cab6b253f67f831017e3f0f5a63c3b | legacy, no longer installed |

Known payload caveats: the McMojave-circle `AUTHORS` file lists third-party icon sources (OSX_ONE, Os-Catalina and others) whose licenses are not verified here. The McMojave-circle payload has 38 dangling symlinks (for example `status/22/weather-clear-night-*.svg`) that are present upstream and left unchanged. The McMojave-circle `index.theme` is unmodified.

## Not bundled

| Pack | Reason |
|---|---|
| 01-Fluent.tar.xz | No license file in the payload (upstream license not verifiable here); install-only |
| 01-Qogir-cursors.tar.xz | No license file in the payload (upstream license not verifiable here); install-only |
| 01-Tela.tar.xz | No license file in the payload (upstream license not verifiable here); install-only |
| 01-Vimix-cursors.tar.xz | No license file in the payload (upstream license not verifiable here); install-only |
| 01-WhiteSur.tar.xz | No license file in the payload (upstream license not verifiable here); install-only |
| BeautySolar.tar-20241114161046.gz | No license file in the payload (upstream license not verifiable here); install-only |
| Bibata-Modern-Ice.tar.xz | No license file in the payload (upstream license not verifiable here); install-only |
| Reversal.tar.xz | GPL-3.0 `COPYING` in payload, but 38 MiB unpacked; not bundled to keep the package small, install-only |
| Sweet-cursors.tar.xz | No license file in the payload (upstream license not verifiable here); install-only |
| capitaine-cursors-light-r4.tar.gz | No license file in the payload (upstream license not verifiable here); install-only |
| oreo-white-cursors.tar.gz | No license file in the payload (upstream license not verifiable here); install-only |
| volantes-cursors.tar.gz | No license file in the payload (upstream license not verifiable here); install-only |
