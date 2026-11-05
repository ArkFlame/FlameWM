//! Shared JSON/text output helpers and the failure exit path.

use crate::canary::Canary;

pub(crate) fn json_str(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(crate) fn fail(message: &str) -> ! {
    eprintln!("CANARY_FAIL {message}");
    std::process::exit(1);
}

pub(crate) fn print_sample(window: u32, x: i16, y: i16, w: u16, h: u16, data: &[u8], json: bool) {
    let (all_zero, pixels, mr, mg, mb) = Canary::pixel_stats(data);
    if json {
        println!(
            "{{\"window\":{window},\"x\":{x},\"y\":{y},\"width\":{w},\"height\":{h},\
             \"pixels\":{pixels},\"all_zero\":{all_zero},\
             \"mean_r\":{mr},\"mean_g\":{mg},\"mean_b\":{mb},\"bytes\":{}}}",
            data.len()
        );
    } else {
        println!(
            "CANARY_SAMPLE window={window} rect={w}x{h}+{x}+{y} pixels={pixels} \
             all_zero={all_zero} mean=#{mr:02x}{mg:02x}{mb:02x} bytes={}",
            data.len()
        );
    }
}

pub(crate) fn emit_results(scenario: &str, results: &[(String, bool, String)], json: bool) -> bool {
    let pass = results.iter().all(|(_, ok, _)| *ok);
    if json {
        let items: Vec<String> = results
            .iter()
            .map(|(n, ok, d)| format!("{{\"name\":{n:?},\"pass\":{ok},\"detail\":{d:?}}}"))
            .collect();
        println!(
            "{{\"scenario\":\"{scenario}\",\"pass\":{pass},\"checks\":[{}]}}",
            items.join(",")
        );
    } else {
        for (name, ok, detail) in results {
            println!("CANARY_{scenario} {name} pass={ok} {detail}");
        }
        println!("CANARY_{scenario} pass={pass} checks={}", results.len());
    }
    pass
}
