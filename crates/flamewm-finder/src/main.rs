fn main() {
    if let Err(error) = flamewm_finder::run() {
        eprintln!("flamewm-finder: {error}");
        std::process::exit(1);
    }
}
