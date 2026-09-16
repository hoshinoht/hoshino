use crate::{
    limits::Limits,
    model::{CountUnit, Diagnostic, DiagnosticSeverity, LanguageLoc, Loc},
    regular_file::{SymlinkPolicy, open_regular},
};
use ignore::{DirEntry, WalkBuilder};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::Path,
    time::{Duration, Instant, SystemTime},
};

const PRUNED_DIRECTORIES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "target",
    "build",
    "dist",
    "node_modules",
    "vendor",
    ".venv",
    "venv",
];
const FINITE_LANGUAGES: &[&str] = &["Rust", "Python", "TypeScript", "JavaScript", "Go"];

pub struct Scan {
    pub lines: BTreeMap<String, u64>,
    pub truncated: bool,
    pub newest_source: Option<SystemTime>,
}

impl Scan {
    pub fn count(&self, l: &str) -> u64 {
        *self.lines.get(l).unwrap_or(&0)
    }

    pub fn into_model(self) -> Loc {
        let total = self.lines.values().sum();
        Loc {
            total,
            unit: CountUnit::Lines,
            truncated: self.truncated,
            by_language: self
                .lines
                .into_iter()
                .map(|(language, lines)| LanguageLoc {
                    language,
                    lines,
                    unit: CountUnit::Lines,
                })
                .collect(),
        }
    }
}

pub fn scan(root: &Path, full: bool, l: &Limits, diags: &mut Vec<Diagnostic>) -> Scan {
    let start = Instant::now();
    let deadline = start + Duration::from_millis(l.scan_elapsed_ms);
    scan_with_clock(root, full, l, diags, || Instant::now() >= deadline)
}

fn scan_with_clock<F>(
    root: &Path,
    full: bool,
    l: &Limits,
    diags: &mut Vec<Diagnostic>,
    mut expired: F,
) -> Scan
where
    F: FnMut() -> bool,
{
    let mut out = Scan {
        lines: BTreeMap::new(),
        truncated: false,
        newest_source: None,
    };
    let mut truncation_reasons = BTreeMap::new();
    let mut files = 0u64;
    let mut read_bytes = 0u64;
    let mut b = WalkBuilder::new(root);
    b.hidden(false)
        .follow_links(false)
        .max_filesize(None)
        .parents(true)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(true)
        .sort_by_file_path(|a, b| a.cmp(b))
        .current_dir(root.to_path_buf())
        .filter_entry(keep_entry);
    for entry in b.build() {
        if expired() {
            truncation_reasons.insert("elapsed", ());
            break;
        }
        let Ok(entry) = entry else { continue };
        if entry.path_is_symlink()
            || !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }
        let Some(lang) = language(entry.path()) else {
            continue;
        };
        if !full && lang == "Other" {
            continue;
        }
        if files >= l.scan_max_files {
            truncation_reasons.insert("file-count", ());
            break;
        }
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.file_type().is_file() {
            continue;
        }
        files += 1;
        if meta.len() > l.scan_max_file_bytes {
            truncation_reasons.insert("file-size", ());
            continue;
        }
        let remaining = l.scan_max_read_bytes.saturating_sub(read_bytes);
        if meta.len() > remaining {
            truncation_reasons.insert("read-bytes", ());
            continue;
        }
        let Ok(mut file) = open_regular(entry.path(), SymlinkPolicy::NoFollow) else {
            continue;
        };
        let Ok(meta) = file.metadata() else { continue };
        if meta.len() > l.scan_max_file_bytes {
            truncation_reasons.insert("file-size", ());
            continue;
        }
        if meta.len() > remaining {
            truncation_reasons.insert("read-bytes", ());
            continue;
        }
        let read = read_source(&mut file, meta.len(), &mut expired);
        read_bytes += read.bytes;
        if read.elapsed {
            truncation_reasons.insert("elapsed", ());
            break;
        }
        if !read.complete || read.binary {
            continue;
        }
        *out.lines.entry(lang.into()).or_default() += read.lines;
        if let Ok(modified) = meta.modified() {
            out.newest_source = Some(
                out.newest_source
                    .map_or(modified, |previous| previous.max(modified)),
            );
        }
    }

    if !truncation_reasons.is_empty() {
        out.truncated = true;
        diags.push(Diagnostic {
            severity: DiagnosticSeverity::Warning,
            code: "scan-truncated".into(),
            message: format!(
                "LOC scan truncated by {} budget(s)",
                truncation_reasons
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            subject: None,
        });
    }

    if !full {
        out.lines
            .retain(|k, _| FINITE_LANGUAGES.contains(&k.as_str()));
    }
    out
}

fn keep_entry(entry: &DirEntry) -> bool {
    if entry.path_is_symlink() {
        return false;
    }
    if !entry
        .file_type()
        .is_some_and(|file_type| file_type.is_dir())
    {
        return true;
    }
    entry
        .file_name()
        .to_str()
        .is_none_or(|name| !PRUNED_DIRECTORIES.contains(&name))
}

struct SourceRead {
    bytes: u64,
    lines: u64,
    binary: bool,
    complete: bool,
    elapsed: bool,
}

fn read_source<F>(file: &mut File, expected: u64, expired: &mut F) -> SourceRead
where
    F: FnMut() -> bool,
{
    let mut buffer = [0u8; 8 * 1024];
    let mut bytes = 0u64;
    let mut lines = 0u64;
    let mut binary = false;
    let mut has_bytes = false;
    let mut ends_with_newline = false;

    while bytes < expected {
        if expired() {
            return SourceRead {
                bytes,
                lines: 0,
                binary: false,
                complete: false,
                elapsed: true,
            };
        }
        let size = (expected - bytes).min(buffer.len() as u64) as usize;
        let read = file.read(&mut buffer[..size]).unwrap_or_default();
        if read == 0 {
            return SourceRead {
                bytes,
                lines: 0,
                binary: false,
                complete: false,
                elapsed: false,
            };
        }
        bytes += read as u64;
        has_bytes = true;
        binary |= buffer[..read].contains(&0);
        lines += buffer[..read].iter().filter(|&&byte| byte == b'\n').count() as u64;
        ends_with_newline = buffer[read - 1] == b'\n';
        if expired() {
            return SourceRead {
                bytes,
                lines: 0,
                binary: false,
                complete: false,
                elapsed: true,
            };
        }
    }

    SourceRead {
        bytes,
        lines: lines + u64::from(has_bytes && !ends_with_newline),
        binary,
        complete: true,
        elapsed: false,
    }
}

fn language(p: &Path) -> Option<&'static str> {
    match p.extension().and_then(|extension| extension.to_str()) {
        Some("rs") => Some("Rust"),
        Some("py") => Some("Python"),
        Some("ts" | "tsx") => Some("TypeScript"),
        Some("js" | "jsx" | "mjs" | "cjs") => Some("JavaScript"),
        Some("go") => Some("Go"),
        Some(_) | None => Some("Other"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        path: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("hoshino-loc-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn write(&self, name: &str, contents: impl AsRef<[u8]>) {
            fs::write(self.path.join(name), contents).unwrap();
        }

        fn write_nested(&self, directory: &str, name: &str, contents: impl AsRef<[u8]>) {
            let directory = self.path.join(directory);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join(name), contents).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn limits() -> Limits {
        Limits {
            scan_max_files: 100,
            scan_max_file_bytes: 1024,
            scan_max_read_bytes: 4096,
            scan_elapsed_ms: 1000,
            ..Limits::default()
        }
    }

    fn truncated_for(diags: &[Diagnostic], reason: &str) -> bool {
        diags
            .iter()
            .any(|diag| diag.code == "scan-truncated" && diag.message.contains(reason))
    }

    #[test]
    fn honors_gitignore_and_prunes_generated_and_vcs_directories() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.path.join(".git")).unwrap();
        fixture.write(".gitignore", "ignored.rs\n");
        fixture.write("ignored.rs", "ignored\n");
        fixture.write("kept.rs", "kept\n");
        for directory in PRUNED_DIRECTORIES {
            fixture.write_nested(directory, "hidden.rs", "not counted\n");
        }

        let mut diags = Vec::new();
        let scan = scan(&fixture.path, true, &limits(), &mut diags);

        assert_eq!(scan.count("Rust"), 1);
        assert!(!scan.truncated);
        assert!(diags.is_empty());
    }

    #[test]
    fn skips_binary_content_without_counting_it() {
        let fixture = Fixture::new();
        fixture.write("binary.rs", b"line\n\0not text\n");
        fixture.write("source.rs", "line\n");

        let mut diags = Vec::new();
        let scan = scan(&fixture.path, false, &limits(), &mut diags);

        assert_eq!(scan.count("Rust"), 1);
        assert!(!scan.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinks_outside_the_root() {
        let fixture = Fixture::new();
        let outside = Fixture::new();
        outside.write("outside.rs", "external\n");
        std::os::unix::fs::symlink(
            outside.path.join("outside.rs"),
            fixture.path.join("external.rs"),
        )
        .unwrap();
        std::os::unix::fs::symlink(&outside.path, fixture.path.join("external-dir")).unwrap();
        fixture.write("inside.rs", "inside\n");

        let mut diags = Vec::new();
        let scan = scan(&fixture.path, false, &limits(), &mut diags);

        assert_eq!(scan.count("Rust"), 1);
        assert!(!scan.truncated);
    }

    #[test]
    fn counts_unicode_and_unterminated_last_lines() {
        let fixture = Fixture::new();
        fixture.write("unicode.rs", "é\n最後の行");
        fixture.write("empty.rs", b"");

        let mut diags = Vec::new();
        let scan = scan(&fixture.path, false, &limits(), &mut diags);

        assert_eq!(scan.count("Rust"), 2);
        assert!(scan.newest_source.is_some());
    }

    #[test]
    fn reports_a_file_size_budget_truncation() {
        let fixture = Fixture::new();
        fixture.write("large.rs", b"12345");
        let mut bounded = limits();
        bounded.scan_max_file_bytes = 4;

        let mut diags = Vec::new();
        let scan = scan(&fixture.path, false, &bounded, &mut diags);

        assert_eq!(scan.count("Rust"), 0);
        assert!(scan.truncated);
        assert!(truncated_for(&diags, "file-size"));
    }

    #[test]
    fn reports_file_count_budget_truncation() {
        let fixture = Fixture::new();
        fixture.write("a.rs", "a\n");
        fixture.write("b.rs", "b\n");
        let mut bounded = limits();
        bounded.scan_max_files = 1;

        let mut diags = Vec::new();
        let scan = scan(&fixture.path, false, &bounded, &mut diags);

        assert_eq!(scan.count("Rust"), 1);
        assert!(scan.truncated);
        assert!(truncated_for(&diags, "file-count"));
    }

    #[test]
    fn reports_aggregate_read_budget_truncation() {
        let fixture = Fixture::new();
        fixture.write("a.rs", "1234");
        fixture.write("b.rs", "b\n");
        let mut bounded = limits();
        bounded.scan_max_read_bytes = 4;

        let mut diags = Vec::new();
        let scan = scan(&fixture.path, false, &bounded, &mut diags);

        assert_eq!(scan.count("Rust"), 1);
        assert!(scan.truncated);
        assert!(truncated_for(&diags, "read-bytes"));
    }

    #[test]
    fn reports_elapsed_budget_with_an_injectable_clock() {
        let fixture = Fixture::new();
        fixture.write("source.rs", "source\n");

        let mut diags = Vec::new();
        let mut checks = 0;
        let scan = scan_with_clock(&fixture.path, false, &limits(), &mut diags, || {
            checks += 1;
            checks > 2
        });

        assert_eq!(scan.count("Rust"), 0);
        assert!(scan.truncated);
        assert!(truncated_for(&diags, "elapsed"));
    }

    #[test]
    fn normal_mode_excludes_other_and_full_mode_counts_other_text() {
        let fixture = Fixture::new();
        fixture.write("source.rs", "rust\n");
        fixture.write("notes.txt", "notes\n");

        let mut normal_diags = Vec::new();
        let normal = scan(&fixture.path, false, &limits(), &mut normal_diags).into_model();
        let mut full_diags = Vec::new();
        let full = scan(&fixture.path, true, &limits(), &mut full_diags).into_model();

        assert_eq!(normal.total, 1);
        assert_eq!(normal.by_language.len(), 1);
        assert_eq!(full.total, 2);
        assert_eq!(
            full.by_language
                .iter()
                .find(|entry| entry.language == "Other")
                .map(|entry| entry.lines),
            Some(1)
        );
        assert!(normal.totals_reconcile());
        assert!(full.totals_reconcile());
    }
}
