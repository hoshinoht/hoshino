//! Run with `cargo run --release --example profile_collect -- /path/to/repo`.
use hoshino::{collect, limits::Limits};
use std::{path::PathBuf, time::Instant};
fn main() {
    let cwd = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let limits = Limits::default();
    let mut diagnostics = Vec::new();
    let start = Instant::now();
    let _ = collect::git::collect(&cwd, &mut diagnostics);
    eprintln!("git: {:?}", start.elapsed());
    if let Some(root) = collect::git::workdir(&cwd) {
        let start = Instant::now();
        let scan = collect::loc::scan(&root, false, &limits, &mut diagnostics);
        eprintln!("loc: {:?}", start.elapsed());
        let start = Instant::now();
        let detection = collect::project::detect(&root, &cwd, &scan);
        eprintln!("detection: {:?}", start.elapsed());
        let start = Instant::now();
        let _ = collect::toolchain::collect(&detection, false, &limits, &mut diagnostics);
        eprintln!("toolchain: {:?}", start.elapsed());
        let start = Instant::now();
        let _ = collect::coverage::collect(
            &root,
            None,
            scan.newest_source,
            scan.truncated,
            &limits,
            &mut diagnostics,
        );
        eprintln!("coverage: {:?}", start.elapsed());
    }
    let start = Instant::now();
    let _ = collect::system::collect();
    eprintln!("system: {:?}", start.elapsed());
    let start = Instant::now();
    let _ = collect::time::collect();
    eprintln!("time: {:?}", start.elapsed());
}
