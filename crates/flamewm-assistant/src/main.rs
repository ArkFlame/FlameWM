fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = if args.first().map(String::as_str) == Some(flamewm_assistant::osd_host::HOST_ARG)
    {
        flamewm_assistant::osd_host::run_host()
    } else if args.first().map(String::as_str) == Some("--popover") {
        match flamewm_assistant::popover_anchor(&args[1..]) {
            Some(anchor) => flamewm_assistant::run_popover(anchor),
            None => Err("usage: flamewm-assistant --popover X Y WIDTH HEIGHT".to_owned()),
        }
    } else {
        flamewm_assistant::run()
    };
    if let Err(error) = result {
        eprintln!("flamewm-assistant: {error}");
        std::process::exit(1);
    }
}
