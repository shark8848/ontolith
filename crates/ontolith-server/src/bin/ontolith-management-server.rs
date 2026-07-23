fn main() {
    if let Err(err) = ontolith_server::management::run() {
        eprintln!("ontolith-management-server failed: {err}");
        std::process::exit(1);
    }
}
