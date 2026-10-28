fn main() {
    if let Err(error) = flamewm_terminal::fetch::cli_main(std::env::args().skip(1)) {
        eprintln!("flamefetch: {error}");
        std::process::exit(1);
    }
}
