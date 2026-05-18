use std::process;

fn main() {
    if let Err(error) = ai_assistant::cli::run_from_env() {
        eprintln!("{error}");
        process::exit(1);
    }
}
