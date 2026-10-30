fn main() {
    let result = if std::env::args().nth(1).as_deref() == Some(flamewm_assistant::osd_host::HOST_ARG) {
        flamewm_assistant::osd_host::run_host()
    } else {
        flamewm_assistant::run()
    };
    if let Err(error) = result {
        eprintln!("flamewm-assistant: {error}");
        std::process::exit(1);
    }
}
