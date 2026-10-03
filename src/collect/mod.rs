//! Bounded, side-effect-free project collection.
pub mod coverage;
pub mod git;
pub mod loc;
pub mod project;
pub mod system;
pub mod time;
pub mod toolchain;

use crate::{
    limits::Limits,
    model::{Context, Diagnostic, Git, Project},
};
use std::path::Path;

#[derive(Debug)]
pub struct ProjectFacts {
    pub context: Context,
    pub git: Option<Git>,
    pub project: Option<Project>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Collect project facts. No command from repository content is ever executed.
pub fn collect(
    root_or_cwd: &Path,
    full: bool,
    limits: &Limits,
    coverage_path: Option<&Path>,
) -> ProjectFacts {
    let mut diagnostics = Vec::new();
    let root = git::workdir(root_or_cwd);
    // LOC and Git status read independent facts. Keep diagnostics private until
    // joining, preserving the serial collector's Git-before-LOC ordering.
    let (git, scan) = std::thread::scope(|scope| {
        let scan = root.as_ref().map(|root| {
            scope.spawn(move || {
                let mut diagnostics = Vec::new();
                let scan = loc::scan(root, full, limits, &mut diagnostics);
                (scan, diagnostics)
            })
        });
        let git = git::collect(root_or_cwd, &mut diagnostics);
        let scan = scan.map(|scan| scan.join().expect("LOC collector panicked"));
        (git, scan)
    });
    let Some(root) = root.filter(|_| git.is_some()) else {
        return ProjectFacts {
            context: Context {
                worktree: None,
                directory: Some(root_or_cwd.display().to_string()),
            },
            git: None,
            project: None,
            diagnostics,
        };
    };
    let (scan, scan_diagnostics) = scan.expect("worktree scan was started");
    diagnostics.extend(scan_diagnostics);
    let detection = project::detect(&root, root_or_cwd, &scan);
    let toolchains = toolchain::collect(&detection, full, limits, &mut diagnostics);
    let coverage = coverage::collect(
        &root,
        coverage_path,
        scan.newest_source,
        scan.truncated,
        limits,
        &mut diagnostics,
    );
    ProjectFacts {
        context: Context {
            worktree: Some(root.display().to_string()),
            directory: Some(root_or_cwd.display().to_string()),
        },
        git,
        project: Some(Project {
            primary_language: detection.primary.map(|x| x.name().into()),
            toolchains,
            loc: Some(scan.into_model()),
            coverage,
        }),
        diagnostics,
    }
}
