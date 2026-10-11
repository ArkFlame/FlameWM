# Bundled icon and cursor themes

Source packs live in `.vendor/cursors-icons-themes-kdestore/` (read-only reference).
Only the packs listed as BUNDLED are copied into `assets/themes/`.

| Pack | Installed as | Upstream | License | Payload evidence | Source archive sha256 | Bundled |
|---|---|---|---|---|---|---|
| 01-McMojave-circle.tar.xz | `assets/themes/icons/McMojave-circle` (light only; `-dark` and `-light` variants dropped) | https://github.com/vinceliuice/McMojave-circle | GPL-3.0 | `COPYING` (GPL v3 text, copied to `LICENSES/McMojave-circle-GPL-3.0.txt`) and `AUTHORS` present in payload | 3c4e4ed0d7b6ceb5377bc8a8b89493b0071665001bf32dd6e8b43bed09e5a741 | yes |
| McMojave-cursors.tar.xz | `assets/themes/cursors/McMojave-cursors` | https://github.com/vinceliuice/McMojave-cursors | UPSTREAM-DECLARED/UNVERIFIED-IN-PAYLOAD (GPL-3.0 per upstream; no license file in payload) | none in payload | 1f5796833bd5c285be970a196afc7fb76e7aec15135acd75a28043ef6d948f94 | yes |

Known payload caveats: the McMojave-circle `AUTHORS` file lists third-party icon sources (OSX_ONE, Os-Catalina and others) whose licenses are not verified here. The McMojave-circle payload has 38 dangling symlinks (for example `status/22/weather-clear-night-*.svg`) that are present upstream and left unchanged. The McMojave-circle `index.theme` is unmodified.

## Not bundled

| Pack | Reason |
|---|---|
| 01-Fluent.tar.xz | Not selected for the bundle |
| 01-Qogir-cursors.tar.xz | Not selected for the bundle |
| 01-Tela.tar.xz | Not selected for the bundle |
| 01-Vimix-cursors.tar.xz | Not selected for the bundle |
| 01-WhiteSur.tar.xz | Not selected for the bundle |
| BeautySolar.tar-20241114161046.gz | Not selected for the bundle |
| Bibata-Modern-Ice.tar.xz | Not selected for the bundle |
| Reversal.tar.xz | Not selected for the bundle |
| Sweet-cursors.tar.xz | Not selected for the bundle |
| candy-icons.tar.xz | Not selected for the bundle |
| capitaine-cursors-light-r4.tar.gz | Not selected for the bundle |
| oreo-white-cursors.tar.gz | Not selected for the bundle |
| volantes-cursors.tar.gz | Not selected for the bundle |
