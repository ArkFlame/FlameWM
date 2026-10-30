fn main() {
    if let Err(error) = flamewm_assistant::run() {
        eprintln!("flamewm-assistant: {error}");
        std::process::exit(1);
    }
}
