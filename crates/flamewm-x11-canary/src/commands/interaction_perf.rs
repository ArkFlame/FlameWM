//! Interaction latency profiling subcommand.

use std::time::Duration;

use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageData, ClientMessageEvent, ConfigureWindowAux, ConnectionExt,
    CreateWindowAux, EventMask, MapState, PropMode, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;

use crate::canary::Canary;
use crate::cli::{arg, has};
use crate::output::fail;

pub(crate) fn canary_now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// J06 deterministic interaction workload: 4 ordinary clients, map,
/// activate A/B/C/D, configure/resize, unmap/remap, destroy/recreate x100
/// cycles, workspace activation when EWMH supported. Emits
/// `ACTION <seq> <kind> <start_ns> <end_ns> <window>` lines, no
/// whole-screen hash (geometry + EWMH atoms only).
pub(crate) fn cmd_interaction_perf(canary: &Canary, args: &[String]) {
    let cycles: u32 = arg(args, "--cycles")
        .map_or(100, |v| {
            v.parse()
                .unwrap_or_else(|_| fail("--cycles must be a number"))
        })
        .clamp(1, 100);
    let json = has(args, "--json");
    let mut seq: u64 = 0;
    let mut failed: Vec<String> = Vec::new();
    let mut emit = |kind: &str, window: u32, start: u64, end: u64| {
        seq += 1;
        println!("ACTION {seq} {kind} {start} {end} {window}");
    };
    // Snapshot pre-existing Flame children (no-Flame-child-dies check).
    let flame_before: Vec<u32> = canary.find("flame").iter().map(|w| w.id).collect();
    let set_prop8 = |wid: u32, prop: Atom, ty: Atom, data: &[u8]| -> bool {
        canary
            .conn
            .change_property8(PropMode::REPLACE, wid, prop, ty, data)
            .is_ok_and(|c| c.check().is_ok())
    };
    let set_prop32 = |wid: u32, prop: Atom, ty: Atom, data: &[u32]| -> bool {
        canary
            .conn
            .change_property32(PropMode::REPLACE, wid, prop, ty, data)
            .is_ok_and(|c| c.check().is_ok())
    };
    let setup = &canary.conn.setup().roots[canary.screen];
    let (depth, visual, white, black) = (
        setup.root_depth,
        setup.root_visual,
        setup.white_pixel,
        setup.black_pixel,
    );
    let wm_protocols = canary.intern("WM_PROTOCOLS").unwrap_or(0);
    let wm_delete = canary.intern("WM_DELETE_WINDOW").unwrap_or(0);
    let utf8 = canary
        .intern("UTF8_STRING")
        .unwrap_or(AtomEnum::STRING.into());
    let net_name = canary
        .intern("_NET_WM_NAME")
        .unwrap_or(AtomEnum::STRING.into());
    let wm_name_atom = canary.intern("WM_NAME").unwrap_or(AtomEnum::STRING.into());
    let wm_class_atom = canary.intern("WM_CLASS").unwrap_or(AtomEnum::STRING.into());
    let cur_desktop_atom = canary.intern("_NET_CURRENT_DESKTOP");
    let num_desktop_atom = canary.intern("_NET_NUMBER_OF_DESKTOPS");
    let ewmh_ws = cur_desktop_atom.is_some() && num_desktop_atom.is_some();

    let labels = ["A", "B", "C", "D"];
    let mut wins: Vec<u32> = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        let wid = canary
            .conn
            .generate_id()
            .unwrap_or_else(|e| fail(&e.to_string()));
        let s = canary_now_ns();
        let title = format!("canary-interaction-perf-{label}");
        let class = format!("canary-interaction-perf-{label}\0interaction-perf\0");
        let create = canary.conn.create_window(
            depth,
            wid,
            canary.root,
            40 + i as i16 * 60,
            40 + i as i16 * 40,
            320,
            240,
            1,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new()
                .background_pixel(white)
                .border_pixel(black)
                .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE),
        );
        let mut ok = create.is_ok_and(|c| c.check().is_ok());
        if ok {
            ok = set_prop8(wid, wm_name_atom, AtomEnum::STRING.into(), title.as_bytes());
        }
        if ok {
            ok = set_prop8(wid, net_name, utf8, title.as_bytes());
        }
        if ok {
            ok = set_prop8(
                wid,
                wm_class_atom,
                AtomEnum::STRING.into(),
                class.as_bytes(),
            );
        }
        if ok && wm_protocols != 0 && wm_delete != 0 {
            let _ = set_prop32(wid, wm_protocols, AtomEnum::ATOM.into(), &[wm_delete]);
        }
        if ok {
            ok = canary.conn.map_window(wid).is_ok_and(|c| c.check().is_ok());
        }
        let e = canary_now_ns();
        emit(&format!("create-{}", label.to_lowercase()), wid, s, e);
        if !ok {
            failed.push(format!("create-{label} failed wid={wid}"));
        }
        wins.push(wid);
    }
    let _ = canary.conn.flush();
    std::thread::sleep(Duration::from_millis(300));
    // Initial contract: clients appear in _NET_CLIENT_LIST.
    let listed = canary.prop_u32(canary.root, "_NET_CLIENT_LIST");
    let ewmh_list = !canary
        .prop_bytes(canary.root, canary.intern("_NET_CLIENT_LIST").unwrap_or(0))
        .is_empty()
        || !listed.is_empty();
    for w in &wins {
        let present = listed.contains(w);
        let s = canary_now_ns();
        emit("check-listed", *w, s, s);
        if ewmh_list && !present {
            failed.push(format!("client {w} missing from _NET_CLIENT_LIST"));
        }
    }
    for cycle in 0..cycles {
        // Activate A/B/C/D in order.
        for (i, w) in wins.clone().iter().enumerate() {
            let s = canary_now_ns();
            let sent = canary.send_active_window(*w).is_ok();
            std::thread::sleep(Duration::from_millis(15));
            let active = canary.active_window();
            let e = canary_now_ns();
            emit(&format!("activate-{}", labels[i].to_lowercase()), *w, s, e);
            if !sent {
                failed.push(format!("cycle {cycle} activate {} send failed", labels[i]));
            } else if active.is_some() && active != Some(*w) {
                failed.push(format!(
                    "cycle {cycle} active follows activation: want {w} got {active:?}"
                ));
            }
        }
        // Configure/resize all; verify geometry observed.
        let w_new: u16 = 320 + ((cycle % 5) * 8) as u16;
        let h_new: u16 = 240 + ((cycle % 4) * 8) as u16;
        for (i, w) in wins.clone().iter().enumerate() {
            let s = canary_now_ns();
            let cfg = canary
                .conn
                .configure_window(
                    *w,
                    &ConfigureWindowAux::new()
                        .width(u32::from(w_new))
                        .height(u32::from(h_new)),
                )
                .is_ok_and(|c| c.check().is_ok());
            std::thread::sleep(Duration::from_millis(10));
            let geom = canary
                .conn
                .get_geometry(*w)
                .ok()
                .and_then(|c| c.reply().ok());
            let e = canary_now_ns();
            emit(&format!("configure-{}", labels[i].to_lowercase()), *w, s, e);
            match geom {
                Some(g) => {
                    if !cfg {
                        failed.push(format!("cycle {cycle} configure {} send failed", labels[i]));
                    }
                    if g.width == 0 || g.height == 0 {
                        failed.push(format!("cycle {cycle} geometry empty {}", labels[i]));
                    }
                }
                None => failed.push(format!("cycle {cycle} geometry {} err", labels[i])),
            }
        }
        // Unmap/remap B at fixed cycles.
        if cycle == 25 {
            let w = wins[1];
            let s = canary_now_ns();
            let ok = canary.conn.unmap_window(w).is_ok_and(|c| c.check().is_ok());
            let _ = canary.conn.flush();
            std::thread::sleep(Duration::from_millis(150));
            let e = canary_now_ns();
            emit("unmap-b", w, s, e);
            if !ok {
                failed.push("unmap-b send failed".to_owned());
            }
            let unmapped = canary
                .by_id(w)
                .is_none_or(|i| i.map_state != u8::from(MapState::VIEWABLE));
            if !unmapped {
                failed.push("unmap-b still viewable".to_owned());
            }
        }
        if cycle == 26 {
            let w = wins[1];
            let s = canary_now_ns();
            let ok = canary.conn.map_window(w).is_ok_and(|c| c.check().is_ok());
            let _ = canary.conn.flush();
            std::thread::sleep(Duration::from_millis(150));
            let e = canary_now_ns();
            emit("remap-b", w, s, e);
            if !ok {
                failed.push("remap-b send failed".to_owned());
            }
            let viewable = canary
                .by_id(w)
                .is_some_and(|i| i.map_state == u8::from(MapState::VIEWABLE));
            if !viewable {
                failed.push("remap-b not viewable".to_owned());
            }
        }
        // Destroy/recreate C at cycle 50.
        if cycle == 50 {
            let w = wins[2];
            let s = canary_now_ns();
            let ok = canary
                .conn
                .destroy_window(w)
                .is_ok_and(|c| c.check().is_ok());
            let _ = canary.conn.flush();
            std::thread::sleep(Duration::from_millis(200));
            let e = canary_now_ns();
            emit("destroy-c", w, s, e);
            if !ok {
                failed.push("destroy-c send failed".to_owned());
            }
            let gone = canary.by_id(w).is_none();
            let list_after = canary.prop_u32(canary.root, "_NET_CLIENT_LIST");
            if ewmh_list && !gone && list_after.contains(&w) {
                failed.push("destroy-c still listed".to_owned());
            }
            // Recreate C.
            let nwid = canary
                .conn
                .generate_id()
                .unwrap_or_else(|e| fail(&e.to_string()));
            let rs = canary_now_ns();
            let title = "canary-interaction-perf-C";
            let class = "canary-interaction-perf-C\0interaction-perf\0";
            let rok = canary
                .conn
                .create_window(
                    depth,
                    nwid,
                    canary.root,
                    160,
                    120,
                    320,
                    240,
                    1,
                    WindowClass::INPUT_OUTPUT,
                    visual,
                    &CreateWindowAux::new()
                        .background_pixel(white)
                        .border_pixel(black)
                        .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE),
                )
                .is_ok_and(|c| c.check().is_ok())
                && set_prop8(
                    nwid,
                    wm_name_atom,
                    AtomEnum::STRING.into(),
                    title.as_bytes(),
                )
                && set_prop8(nwid, net_name, utf8, title.as_bytes())
                && set_prop8(
                    nwid,
                    wm_class_atom,
                    AtomEnum::STRING.into(),
                    class.as_bytes(),
                )
                && canary
                    .conn
                    .map_window(nwid)
                    .is_ok_and(|c| c.check().is_ok());
            let _ = canary.conn.flush();
            std::thread::sleep(Duration::from_millis(200));
            let re = canary_now_ns();
            emit("recreate-c", nwid, rs, re);
            if !rok {
                failed.push("recreate-c failed".to_owned());
            }
            wins[2] = nwid;
        }
        // Workspace activation when EWMH supported.
        if ewmh_ws
            && cycle == 75
            && let Some(atom) = cur_desktop_atom
        {
            let target: u32 = 0;
            let ev = ClientMessageEvent {
                response_type: 33,
                format: 32,
                sequence: 0,
                window: canary.root,
                type_: atom,
                data: ClientMessageData::from([target, CURRENT_TIME, 0, 0, 0]),
            };
            let s = canary_now_ns();
            let ok = canary
                .conn
                .send_event(false, canary.root, EventMask::SUBSTRUCTURE_REDIRECT, ev)
                .is_ok_and(|c| c.check().is_ok());
            let _ = canary.conn.flush();
            std::thread::sleep(Duration::from_millis(100));
            let e = canary_now_ns();
            emit("workspace-activate", canary.root, s, e);
            if !ok {
                failed.push("workspace-activate send failed".to_owned());
            }
        }
    }
    // Cleanup: destroy our 4 clients.
    for (i, w) in wins.iter().enumerate() {
        let s = canary_now_ns();
        let _ = canary.conn.destroy_window(*w);
        let _ = canary.conn.flush();
        let e = canary_now_ns();
        emit(&format!("cleanup-{}", labels[i].to_lowercase()), *w, s, e);
    }
    std::thread::sleep(Duration::from_millis(200));
    // No Flame child dies: pre-existing flame windows still present.
    let mut flame_lost = 0;
    for id in &flame_before {
        if canary.by_id(*id).is_none() {
            flame_lost += 1;
            failed.push(format!("flame child {id} died"));
        }
    }
    let pass = failed.is_empty();
    if json {
        let fails: Vec<String> = failed.iter().map(|f| format!("{f:?}")).collect();
        println!(
            "{{\"scenario\":\"interaction-perf\",\"pass\":{pass},\"cycles\":{cycles},\
             \"actions\":{seq},\"flame_before\":{},\"flame_lost\":{flame_lost},\
             \"ewmh_list\":{ewmh_list},\"ewmh_ws\":{ewmh_ws},\"failures\":[{}]}}",
            flame_before.len(),
            fails.join(",")
        );
    } else {
        for f in &failed {
            println!("CANARY_INTERACTION_PERF fail {f}");
        }
        println!(
            "CANARY_INTERACTION_PERF pass={pass} cycles={cycles} actions={seq} \
             flame_before={} flame_lost={flame_lost} ewmh_list={ewmh_list} ewmh_ws={ewmh_ws}",
            flame_before.len()
        );
    }
    if !pass {
        std::process::exit(1);
    }
}
