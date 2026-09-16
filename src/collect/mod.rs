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
    let git = git::collect(root_or_cwd, &mut diagnostics);
    let Some(root) = git.as_ref().and_then(|_| git::workdir(root_or_cwd)) else {
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
    let scan = loc::scan(&root, full, limits, &mut diagnostics);
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
