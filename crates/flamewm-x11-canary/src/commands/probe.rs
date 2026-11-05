//! Basic probe subcommands: list, geometry, map state, click, motion, sampling.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::MapState;

use crate::canary::{Canary, WinInfo};
use crate::cli::{arg, arg_num, has};
use crate::output::{fail, print_sample};

pub(crate) fn viewable_count(hits: &[WinInfo]) -> usize {
    hits.iter()
        .filter(|w| w.map_state == u8::from(MapState::VIEWABLE))
        .count()
}

pub(crate) fn resolve_target(canary: &Canary, args: &[String]) -> WinInfo {
    if let Some(id) = arg(args, "--window") {
        let id: u32 = id
            .parse()
            .unwrap_or_else(|_| fail("--window must be a number"));
        return canary
            .by_id(id)
            .unwrap_or_else(|| fail(&format!("window {id} not found")));
    }
    if let Some(name) = arg(args, "--name") {
        let mut hits = canary.find(&name);
        if hits.is_empty() {
            fail(&format!("no window matches {name:?}"));
        }
        hits.sort_by_key(|w| w.map_state != u8::from(MapState::VIEWABLE));
        return hits.remove(0);
    }
    fail("need --window <id> or --name <substr>");
}

pub(crate) fn cmd_list(canary: &Canary, args: &[String]) {
    let wins = match arg(args, "--name") {
        Some(n) => canary.find(&n),
        None => canary.all(),
    };
    if has(args, "--json") {
        let items: Vec<String> = wins.iter().map(WinInfo::to_json).collect();
        println!("{{\"windows\":[{}]}}", items.join(","));
    } else {
        for w in &wins {
            println!(
                "win id={} parent={} geom={}x{}+{}+{} map={} name={:?} class={:?}",
                w.id,
                w.parent,
                w.width,
                w.height,
                w.x,
                w.y,
                w.map_name(),
                w.name,
                w.class
            );
        }
        println!("CANARY_LIST count={}", wins.len());
    }
}

pub(crate) fn cmd_geometry(canary: &Canary, args: &[String]) {
    let target = resolve_target(canary, args);
    if has(args, "--json") {
        println!("{}", target.to_json());
    } else {
        println!(
            "CANARY_GEOMETRY id={} geom={}x{}+{}+{} map={} name={:?}",
            target.id,
            target.width,
            target.height,
            target.x,
            target.y,
            target.map_name(),
            target.name
        );
    }
}

pub(crate) fn cmd_map_state(canary: &Canary, args: &[String]) {
    let target = resolve_target(canary, args);
    if has(args, "--json") {
        println!(
            "{{\"id\":{},\"map_state\":\"{}\",\"viewable\":{}}}",
            target.id,
            target.map_name(),
            target.map_state == u8::from(MapState::VIEWABLE)
        );
    } else {
        println!(
            "CANARY_MAP_STATE id={} map={} viewable={}",
            target.id,
            target.map_name(),
            target.map_state == u8::from(MapState::VIEWABLE)
        );
    }
}

pub(crate) fn cmd_click(canary: &Canary, args: &[String]) {
    let target = resolve_target(canary, args);
    let x: i16 = arg_num(args, "--x", (target.width / 2) as i16);
    let y: i16 = arg_num(args, "--y", (target.height / 2) as i16);
    canary
        .send_button(target.id, x, y, true)
        .unwrap_or_else(|e| fail(&e));
    canary
        .send_button(target.id, x, y, false)
        .unwrap_or_else(|e| fail(&e));
    println!("CANARY_CLICK id={} x={x} y={y} ok=true", target.id);
}

pub(crate) fn cmd_motion(canary: &Canary, args: &[String]) {
    let target = resolve_target(canary, args);
    let x: i16 = arg_num(args, "--x", (target.width / 2) as i16);
    let y: i16 = arg_num(args, "--y", (target.height / 2) as i16);
    canary
        .send_motion(target.id, x, y)
        .unwrap_or_else(|e| fail(&e));
    println!("CANARY_MOTION id={} x={x} y={y} ok=true", target.id);
}

pub(crate) fn cmd_sample(canary: &Canary, args: &[String]) {
    let target = resolve_target(canary, args);
    let w: u16 = arg_num(args, "--w", target.width.min(64));
    let h: u16 = arg_num(args, "--h", target.height.min(64));
    let x: i16 = arg_num(args, "--x", 0);
    let y: i16 = arg_num(args, "--y", 0);
    let data = canary
        .sample(target.id, x, y, w.max(1), h.max(1))
        .unwrap_or_else(|e| fail(&e));
    print_sample(target.id, x, y, w, h, &data, has(args, "--json"));
}

pub(crate) fn cmd_sample_root(canary: &Canary, args: &[String]) {
    let setup = &canary.conn.setup().roots[canary.screen];
    let sw = setup.width_in_pixels;
    let sh = setup.height_in_pixels;
    let w: u16 = arg_num(args, "--w", sw.min(256));
    let h: u16 = arg_num(args, "--h", sh.min(256));
    let data = canary
        .sample(canary.root, 0, 0, w.max(1), h.max(1))
        .unwrap_or_else(|e| fail(&e));
    print_sample(canary.root, 0, 0, w, h, &data, has(args, "--json"));
}

pub(crate) fn cmd_stacking(canary: &Canary, args: &[String]) {
    let order = canary.stacking_order().unwrap_or_else(|e| fail(&e));
    if has(args, "--json") {
        let items: Vec<String> = order.iter().map(std::string::ToString::to_string).collect();
        println!(
            "{{\"stacking\":[{}],\"count\":{}}}",
            items.join(","),
            order.len()
        );
    } else {
        for (depth, id) in order.iter().enumerate() {
            println!("CANARY_STACK depth={depth} id={id}");
        }
        println!("CANARY_STACKING count={}", order.len());
    }
}

pub(crate) fn cmd_root_geometry(canary: &Canary, args: &[String]) {
    let (w, h) = canary.root_geometry();
    if has(args, "--json") {
        println!("{{\"root\":{},\"width\":{w},\"height\":{h}}}", canary.root);
    } else {
        println!("CANARY_ROOT_GEOMETRY root={} geom={w}x{h}", canary.root);
    }
}

pub(crate) fn cmd_root_shot(canary: &Canary, args: &[String]) {
    let (sw, sh) = canary.root_geometry();
    let w: u16 = arg_num(args, "--w", sw.min(256));
    let h: u16 = arg_num(args, "--h", sh.min(256));
    let data = canary
        .sample(canary.root, 0, 0, w.max(1), h.max(1))
        .unwrap_or_else(|e| fail(&e));
    print_sample(canary.root, 0, 0, w, h, &data, has(args, "--json"));
}

pub(crate) fn cmd_click_at(canary: &Canary, args: &[String]) {
    let id: u32 = arg(args, "--window")
        .unwrap_or_else(|| fail("click-at needs --window <id>"))
        .parse()
        .unwrap_or_else(|_| fail("--window must be a number"));
    let info = canary
        .by_id(id)
        .unwrap_or_else(|| fail(&format!("window {id} not found")));
    let x: i16 = arg(args, "--x")
        .unwrap_or_else(|| fail("click-at needs --x <n>"))
        .parse()
        .unwrap_or_else(|_| fail("--x must be a number"));
    let y: i16 = arg(args, "--y")
        .unwrap_or_else(|| fail("click-at needs --y <n>"))
        .parse()
        .unwrap_or_else(|_| fail("--y must be a number"));
    let button: u8 = arg(args, "--button").map_or(3, |v| {
        v.parse()
            .unwrap_or_else(|_| fail("--button must be 1, 2, or 3"))
    });
    if !(1..=3).contains(&button) {
        fail("--button must be 1, 2, or 3");
    }
    if x < 0 || y < 0 || x >= info.width as i16 || y >= info.height as i16 {
        fail(&format!(
            "click-at coords ({x},{y}) outside window {} geometry {}x{}",
            id, info.width, info.height
        ));
    }
    canary
        .send_button_detail(id, x, y, true, button)
        .unwrap_or_else(|e| fail(&e));
    canary
        .send_button_detail(id, x, y, false, button)
        .unwrap_or_else(|e| fail(&e));
    println!("CANARY_CLICK_AT id={id} x={x} y={y} button={button} ok=true");
}
