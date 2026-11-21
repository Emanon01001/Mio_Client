fn main() -> std::process::ExitCode {
    mio_signature_check::cli::run(std::env::args_os().skip(1)).into()
}
