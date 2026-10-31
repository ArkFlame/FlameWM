fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match flamewm_archiver_app::parse_args(&args) {
        flamewm_archiver_app::Command::Usage => {
            eprintln!("usage: flamewm-archiver <archive.zip|archive.tar|archive.tar.gz>");
            std::process::exit(2);
        }
        flamewm_archiver_app::Command::Open(path) => match flamewm_archiver_app::listing(&path) {
            Ok(lines) => {
                for line in lines {
                    println!("{line}");
                }
            }
            Err(error) => {
                eprintln!("flamewm-archiver: {error}");
                std::process::exit(1);
            }
        },
    }
}
