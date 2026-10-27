fn main() {
    if let Err(error) = flamewm_task_manager::run() {
        eprintln!("flamewm-task-manager: {error}");
        std::process::exit(1);
    }
}
