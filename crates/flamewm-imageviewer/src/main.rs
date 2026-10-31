fn main() {
    if let Err(error) = flamewm_imageviewer::run() {
        eprintln!("flamewm-imageviewer: {error}");
        std::process::exit(1);
    }
}
