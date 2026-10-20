use std::env;
use std::fs;

use flamewm_render_core::decode;

fn main() {
    let path = env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: flamewm-render-inspect file.rwr");
        std::process::exit(2);
    });
    let bytes = fs::read(&path).unwrap_or_else(|error| {
        eprintln!("flamewm-render-inspect: failed to read {path}: {error}");
        std::process::exit(1);
    });
    let document = decode(&bytes).unwrap_or_else(|error| {
        eprintln!("flamewm-render-inspect: {error}");
        std::process::exit(1);
    });
    // Report the actual decoded file's version, not a stale hard-coded format.
    let format_version = u16::from_le_bytes([bytes[4], bytes[5]]);
    println!("format=RWRB/{format_version}");
    println!("assets={}", document.assets.len());
    println!("fingerprint={:016x}", document.source_fingerprint);
    println!("nodes={}", document.nodes.len());
    println!("variables={}", document.variables.len());
    for (index, variable) in document.variables.iter().enumerate() {
        println!(
            "var[{index}] {} = #{:02x}{:02x}{:02x}{:02x}",
            variable.name,
            variable.default.r,
            variable.default.g,
            variable.default.b,
            variable.default.a
        );
    }
    for (index, node) in document.nodes.iter().enumerate() {
        println!(
            "node[{index}] kind={:?} parent={:?} id={:?} action={:?} text={:?}",
            node.kind, node.parent, node.id, node.action, node.text
        );
    }
}
