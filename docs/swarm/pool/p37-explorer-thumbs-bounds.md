# Unit p37-explorer-thumbs-bounds
STATUS: DONE

Crates flamewm-explorer and flamewm-finder: thumbnail and directory caches: bound size, cancel work for hidden/scrolled-away items, no unbounded threads.

## Result

- `crates/flamewm-explorer/src/thumbs.rs:34-39,607-714,731-781` — finding: already bounded (256 entries / 256 KiB byte LRU, 25 MiB / 25 MPx source caps) with staged `wanted()` cancellation for scrolled-away rows; `HiddenCache` already implements the 30 s grace and queue drop but was unreachable from production. change: none here. risk: none.
- `crates/flamewm-explorer/src/workers.rs:11-51,67-91` — finding: one thumb decoder and fixed-size `JobPool` lanes; no per-row thread spawn. change: none. risk: none.
- `crates/flamewm-finder/src/index.rs:14-15,199-235,307-337` and `watch.rs:17,84` — finding: bounded walk/cache (`MAX_ENTRIES=250k`, `MAX_DEPTH=12`, `WALK_BUDGET`) and `MAX_WATCHES=4096`; `preview.rs` reads only a bounded prefix. change: none. risk: none.
- `crates/flamewm-explorer/src/main.rs:218-219,290-291,504` — finding: `UiControllerEvent::Visibility` was a no-op, so a covered window kept decoded pixels and let in-flight decodes finish. change: added `WorkerResult::HiddenCacheDue`, `hidden`/`hidden_timer` app fields, and forwarded the event to `app.visibility`. **UNBUILT**. risk: one harmless deadline wake after an early uncover; no visible-state change.
- `crates/flamewm-explorer/src/app/lifecycle.rs:2,8,44-61` — finding: no visibility handler existed. change: added `visibility()`, which starts the grace on obscure and arms one wake through a single reused `spawn_frame_ticker` lane (no per-event thread, no polling), and cancels on uncover. **UNBUILT**. risk: one long-lived ticker thread for the process; a refused thread degrades to no drop, exactly as the existing glide ticker does.
- `crates/flamewm-explorer/src/app/input.rs:497-500` — finding: no wake path applied the hidden policy. change: `HiddenCacheDue` calls `hidden.check(now, thumbs, queue)`, clearing the cache and abandoning queued/in-flight decodes. **UNBUILT**. risk: rows decode again on uncover; returned pixels are identical.
- Evidence: `.vendor/cosmic-epoch/cosmic-files/src/thumbnail_cacher.rs` and `.vendor/LingmoOS-main/fm/thumbnailer/thumbnailcache.h` show disk-keyed thumbnail caches with no hidden-window eviction precedent; local `thumbs/tests/mod.rs:404-438` already covers `HiddenCache` grace/uncover/drop. No `cargo` build or test was run (brief forbids it).

Every production edit above is UNBUILT; the orchestrator builds and tests after the batch lands.

PASS — the bounded LRU, source caps and scrolled-away cancellation were already present and were confirmed, and the one real gap (a hidden-window policy that was implemented but never wired) is closed: visibility now feeds `HiddenCache`, a single reused timer lane arms one deadline wake, and that wake clears the cache and abandons queued decodes without per-event threads, polling, or any visible pixel change.
