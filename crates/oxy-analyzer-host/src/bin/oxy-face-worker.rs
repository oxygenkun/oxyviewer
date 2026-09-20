fn main() {
    if oxy_analyzer_host::serve_worker().is_err() {
        std::process::exit(1);
    }
}
