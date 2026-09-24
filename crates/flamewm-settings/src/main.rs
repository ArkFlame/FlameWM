fn main() {
    let options = match flamewm_settings::runtime::launch::parse_args(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("flamewm-settings: {error}");
            std::process::exit(2);
        }
    };
    if let Err(error) = flamewm_settings::runtime::run(options) {
        eprintln!("flamewm-settings: {error}");
        std::process::exit(1);
    }
}
