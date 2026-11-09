use std::env;
use std::fs;

use flamewm_render_core::{RuntimeDocument, decode};
use flamewm_render_x11::{X11Config, run};

/// One line the Settings Check button parses: `backend=... device=... reason=...`.
fn probe_line(result: Result<String, String>) -> String {
    match result {
        Ok(device) => format!("backend=opengl device={device} reason=none"),
        Err(reason) => format!("backend=cpu device=none reason={reason}"),
    }
}

fn main() {
    if env::args().nth(1).as_deref() == Some("--gpu-probe") {
        println!("{}", probe_line(flamewm_render_x11::probe_gpu_renderer()));
        return;
    }
    if let Err(error) = execute() {
        eprintln!("flamewm-render-x11: {error}");
        std::process::exit(1);
    }
}

fn execute() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let mut path = None;
    let mut config = X11Config::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--width" => {
                config.width = args
                    .next()
                    .ok_or("--width requires a value")?
                    .parse()
                    .map_err(|_| "invalid --width")?
            }
            "--height" => {
                config.height = args
                    .next()
                    .ok_or("--height requires a value")?
                    .parse()
                    .map_err(|_| "invalid --height")?
            }
            "--title" => config.title = args.next().ok_or("--title requires a value")?,
            "-h" | "--help" => {
                println!(
                    "usage: flamewm-render-x11 [--width N] [--height N] [--title TEXT] file.rwr"
                );
                return Ok(());
            }
            value if value.starts_with('-') => return Err(format!("unknown option '{value}'")),
            value => {
                if path.replace(value.to_string()).is_some() {
                    return Err("only one .rwr input is accepted".to_string());
                }
            }
        }
    }
    let path = path.ok_or("missing .rwr input")?;
    let bytes = fs::read(&path).map_err(|error| format!("failed to read {path}: {error}"))?;
    let compiled = decode(&bytes)?;
    let runtime = RuntimeDocument::new(compiled)?;
    run(runtime, config)
}

#[cfg(test)]
mod probe_line_tests {
    use super::probe_line;

    #[test]
    fn probe_line_reports_the_device_or_the_fallback_reason() {
        assert_eq!(
            probe_line(Ok("NVIDIA RTX".to_owned())),
            "backend=opengl device=NVIDIA RTX reason=none"
        );
        assert_eq!(
            probe_line(Err("libEGL.so.1 missing".to_owned())),
            "backend=cpu device=none reason=libEGL.so.1 missing"
        );
    }
}
