use hcdformat_xsdgen::{check_repository, write_repository};
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!("usage: hcdformat-xsdgen (--check|--write) [repository-root]");
    std::process::exit(2);
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let mode = arguments.next().unwrap_or_else(|| usage());
    let root = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
    if arguments.next().is_some() {
        usage();
    }

    let result = match mode.as_str() {
        "--check" => check_repository(&root),
        "--write" => write_repository(&root),
        _ => usage(),
    };
    if let Err(error) = result {
        eprintln!("hcdformat-xsdgen: {error}");
        std::process::exit(1);
    }
}
