use super::loc::Scan;
use std::{fs, path::Path};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Language {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Go,
}

impl Language {
    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::TypeScript => "TypeScript",
            Self::JavaScript => "JavaScript",
            Self::Go => "Go",
        }
    }
}

#[derive(Debug)]
pub struct Detection {
    pub primary: Option<Language>,
    pub detected: Vec<Language>,
    pub bun: bool,
}

pub fn detect(root: &Path, cwd: &Path, scan: &Scan) -> Detection {
    let rust_distance = manifest_distance(root, cwd, Language::Rust);
    let python_distance = manifest_distance(root, cwd, Language::Python);
    let tsconfig_distance = manifest_distance(root, cwd, Language::TypeScript);
    let js_distance = manifest_distance(root, cwd, Language::JavaScript);
    let go_distance = manifest_distance(root, cwd, Language::Go);
    let ts_lines = scan.count("TypeScript");
    let js_lines = scan.count("JavaScript");
    let ts_project = tsconfig_distance.is_some() || (js_distance.is_some() && ts_lines > js_lines);
    let distances = [
        (Language::Rust, rust_distance),
        (Language::Python, python_distance),
        (
            Language::TypeScript,
            tsconfig_distance.or_else(|| (ts_lines > js_lines).then_some(js_distance).flatten()),
        ),
        (
            Language::JavaScript,
            (!ts_project).then_some(js_distance).flatten(),
        ),
        (Language::Go, go_distance),
    ];
    let found = [
        (
            Language::Rust,
            rust_distance.is_some() || scan.count("Rust") > 0,
        ),
        (
            Language::Python,
            python_distance.is_some() || scan.count("Python") > 0,
        ),
        (Language::TypeScript, ts_project || ts_lines > 0),
        (
            Language::JavaScript,
            (!ts_project && js_distance.is_some()) || js_lines > 0,
        ),
        (Language::Go, go_distance.is_some() || scan.count("Go") > 0),
    ]
    .into_iter()
    .filter_map(|(language, present)| present.then_some(language))
    .collect::<Vec<_>>();
    let primary = found.iter().copied().min_by_key(|language| {
        let distance = distances
            .iter()
            .find_map(|(candidate, distance)| (*candidate == *language).then_some(*distance))
            .flatten()
            .unwrap_or(usize::MAX);
        (
            distance,
            std::cmp::Reverse(scan.count(language.name())),
            *language,
        )
    });

    Detection {
        primary,
        bun: bun_distance(root, cwd).is_some()
            && found
                .iter()
                .any(|language| matches!(language, Language::TypeScript | Language::JavaScript)),
        detected: found,
    }
}

fn manifest_distance(root: &Path, cwd: &Path, language: Language) -> Option<usize> {
    let markers: &[&str] = match language {
        Language::Rust => &["Cargo.toml"],
        Language::Python => &["pyproject.toml", "uv.lock", "Pipfile"],
        Language::TypeScript => &["tsconfig.json"],
        Language::JavaScript => &[
            "package.json",
            "package-lock.json",
            "yarn.lock",
            "pnpm-lock.yaml",
        ],
        Language::Go => &["go.mod"],
    };
    ancestors(root, cwd)
        .enumerate()
        .find_map(|(distance, path)| {
            (markers.iter().any(|marker| path.join(marker).is_file())
                || (language == Language::Python && has_requirements_file(path)))
            .then_some(distance)
        })
}

fn has_requirements_file(path: &Path) -> bool {
    fs::read_dir(path).ok().is_some_and(|entries| {
        entries.flatten().any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("requirements") && name.ends_with(".txt") && entry.path().is_file()
        })
    })
}

fn bun_distance(root: &Path, cwd: &Path) -> Option<usize> {
    ancestors(root, cwd)
        .enumerate()
        .find_map(|(distance, path)| {
            ["bun.lock", "bun.lockb", "bunfig.toml"]
                .iter()
                .any(|marker| path.join(marker).is_file())
                .then_some(distance)
        })
}

fn ancestors<'a>(root: &'a Path, cwd: &'a Path) -> impl Iterator<Item = &'a Path> {
    let mut paths = Vec::new();
    let mut path = cwd;
    loop {
        paths.push(path);
        if path == root {
            break;
        }
        let Some(parent) = path.parent() else { break };
        path = parent;
    }
    paths.into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::BTreeMap,
        sync::atomic::{AtomicUsize, Ordering},
    };

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn fixture() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "hoshino-project-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn scan(lines: &[(&str, u64)]) -> Scan {
        Scan {
            lines: lines
                .iter()
                .map(|(name, count)| ((*name).into(), *count))
                .collect::<BTreeMap<_, _>>(),
            truncated: false,
            newest_source: None,
        }
    }

    #[test]
    fn nearest_marker_wins_before_loc() {
        let root = fixture();
        let cwd = root.join("nested");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(root.join("Cargo.toml"), "").unwrap();
        fs::write(cwd.join("pyproject.toml"), "").unwrap();
        assert_eq!(
            detect(&root, &cwd, &scan(&[("Rust", 100), ("Python", 1)])).primary,
            Some(Language::Python)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn python_markers_include_uv_pipfile_and_requirements() {
        for marker in ["uv.lock", "Pipfile", "requirements-dev.txt"] {
            let root = fixture();
            fs::write(root.join(marker), "").unwrap();
            assert_eq!(
                detect(&root, &root, &scan(&[])).primary,
                Some(Language::Python)
            );
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn bare_package_is_js_unless_typescript_evidence_wins() {
        let root = fixture();
        fs::write(root.join("package.json"), "{}").unwrap();
        assert_eq!(
            detect(&root, &root, &scan(&[])).primary,
            Some(Language::JavaScript)
        );
        assert_eq!(
            detect(&root, &root, &scan(&[("TypeScript", 3), ("JavaScript", 2)])).primary,
            Some(Language::TypeScript)
        );
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        assert_eq!(
            detect(&root, &root, &scan(&[])).primary,
            Some(Language::TypeScript)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn polyglot_ties_follow_fixed_language_order_and_bun_is_not_a_language() {
        let root = fixture();
        for marker in [
            "Cargo.toml",
            "pyproject.toml",
            "tsconfig.json",
            "package.json",
            "go.mod",
            "bunfig.toml",
        ] {
            fs::write(root.join(marker), "").unwrap();
        }
        let detection = detect(
            &root,
            &root,
            &scan(&[
                ("Rust", 10),
                ("Python", 10),
                ("TypeScript", 10),
                ("JavaScript", 10),
                ("Go", 10),
            ]),
        );
        assert_eq!(detection.primary, Some(Language::Rust));
        assert_eq!(
            detection.detected,
            vec![
                Language::Rust,
                Language::Python,
                Language::TypeScript,
                Language::JavaScript,
                Language::Go
            ]
        );
        assert!(detection.bun);
        let _ = fs::remove_dir_all(root);
    }
}
