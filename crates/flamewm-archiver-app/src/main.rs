use flamewm_archiver_app::{Command, window::Start};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let start = match flamewm_archiver_app::parse_args(&args) {
        Command::Usage => {
            eprintln!(
                "usage: flamewm-archiver <archive.zip|archive.tar|archive.tar.gz|archive.tar.xz|archive.tar.bz2>\n       flamewm-archiver --create <out.zip|out.tar|out.tar.gz|out.tar.xz|out.tar.bz2> <path>..."
            );
            std::process::exit(2);
        }
        Command::Open(path) => Start::Browse(path),
        Command::Create { out, inputs } => Start::Create { out, inputs },
    };
    if let Err(error) = flamewm_archiver_app::window::run(start) {
        eprintln!("flamewm-archiver: {error}");
        std::process::exit(1);
    }
}
