# Unit p87-desktop-rss-regression
STATUS: OPEN

flamewm-desktop RSS grew about +5.7 MB (heap 9.2->11.4 MB, new 3.6 MB anon) since commit 408c614e. Read git diff 408c614e HEAD for crates/flamewm-desktop/src, flamewm-image-core, flamewm-render-core, flamewm-ui-x11, flamewm-integrations-linux icons and platform. List every change that allocates at startup (Vec::with_capacity, vec![0; n], caches pre-filled, threads spawned eagerly each with its own resolver/index/decoder, bundled assets decoded eagerly). Rank by probable bytes. Fix the top one (lazy construct, as done for the shell icon loader in crates/flamewm-shell/src/icon_loader.rs) with at most 2 edits. Output identical.
