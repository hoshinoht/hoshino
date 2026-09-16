use crate::model::{Diagnostic, DiagnosticSeverity, Git, GitHead};
use git2::{BranchType, ErrorCode, Oid, Repository, Status, StatusOptions};
use std::path::{Path, PathBuf};

pub fn workdir(path: &Path) -> Option<PathBuf> {
    Repository::discover(path)
        .ok()?
        .workdir()
        .map(Path::to_path_buf)
}

pub fn collect(path: &Path, diags: &mut Vec<Diagnostic>) -> Option<Git> {
    let repo = match Repository::discover(path) {
        Ok(repo) => repo,
        Err(error) if error.code() == ErrorCode::NotFound => return None,
        Err(error) => {
            diags.push(d("git-discover", error.message()));
            return None;
        }
    };

    let (head, branch_name, head_target) = match repo.head() {
        Ok(reference) if reference.is_branch() => {
            let name = match reference.shorthand() {
                Ok(name) => name.to_owned(),
                Err(error) => {
                    diags.push(d("git-head", error.message()));
                    return None;
                }
            };
            (
                GitHead::Branch { name: name.clone() },
                Some(name),
                reference.target(),
            )
        }
        Ok(reference) => {
            let Some(commit) = reference.target() else {
                diags.push(d("git-head", "detached HEAD has no commit target"));
                return None;
            };
            (
                GitHead::Detached {
                    commit: commit.to_string(),
                },
                None,
                Some(commit),
            )
        }
        Err(error) if error.code() == ErrorCode::UnbornBranch => (GitHead::Unborn, None, None),
        Err(error) => {
            diags.push(d("git-head", error.message()));
            return None;
        }
    };

    let mut options = StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true);
    let statuses = match repo.statuses(Some(&mut options)) {
        Ok(statuses) => statuses,
        Err(error) => {
            diags.push(d("git-status", error.message()));
            return None;
        }
    };

    let (mut staged, mut unstaged, mut conflicts) = (0_u32, 0_u32, 0_u32);
    for entry in statuses.iter() {
        let status = entry.status();
        if status.intersects(Status::CONFLICTED) {
            conflicts = conflicts.saturating_add(1);
            continue;
        }
        if status.intersects(
            Status::INDEX_NEW
                | Status::INDEX_MODIFIED
                | Status::INDEX_DELETED
                | Status::INDEX_RENAMED
                | Status::INDEX_TYPECHANGE,
        ) {
            staged = staged.saturating_add(1);
        }
        if status.intersects(
            Status::WT_NEW
                | Status::WT_MODIFIED
                | Status::WT_DELETED
                | Status::WT_RENAMED
                | Status::WT_TYPECHANGE,
        ) {
            unstaged = unstaged.saturating_add(1);
        }
    }

    let (upstream, ahead) = upstream(&repo, branch_name.as_deref(), head_target, diags);
    Some(Git {
        head,
        dirty: staged > 0 || unstaged > 0 || conflicts > 0,
        staged,
        unstaged,
        conflicts,
        ahead,
        upstream,
    })
}

fn upstream(
    repo: &Repository,
    branch_name: Option<&str>,
    head_target: Option<Oid>,
    diags: &mut Vec<Diagnostic>,
) -> (Option<String>, Option<u32>) {
    let Some(branch_name) = branch_name else {
        return (None, None);
    };

    let branch = match repo.find_branch(branch_name, BranchType::Local) {
        Ok(branch) => branch,
        Err(error) => {
            diags.push(d("git-upstream", error.message()));
            return (None, None);
        }
    };
    let upstream = match branch.upstream() {
        Ok(upstream) => upstream,
        Err(error) if error.code() == ErrorCode::NotFound => return (None, None),
        Err(error) => {
            diags.push(d("git-upstream", error.message()));
            return (None, None);
        }
    };
    let name = match upstream.name() {
        Ok(Some(name)) => name.to_owned(),
        Ok(None) => {
            diags.push(d("git-upstream", "upstream name is not valid UTF-8"));
            return (None, None);
        }
        Err(error) => {
            diags.push(d("git-upstream", error.message()));
            return (None, None);
        }
    };

    let ahead = match (head_target, upstream.get().target()) {
        (Some(local), Some(remote)) => match repo.graph_ahead_behind(local, remote) {
            Ok((ahead, _)) => Some(u32::try_from(ahead).unwrap_or(u32::MAX)),
            Err(error) => {
                diags.push(d("git-ahead", error.message()));
                None
            }
        },
        _ => None,
    };
    (Some(name), ahead)
}

fn d(code: &str, msg: &str) -> Diagnostic {
    Diagnostic {
        severity: DiagnosticSeverity::Warning,
        code: code.into(),
        message: msg.into(),
        subject: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{RepositoryInitOptions, Signature, build::CheckoutBuilder};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_REPO: AtomicU64 = AtomicU64::new(0);

    struct TempRepo {
        path: PathBuf,
    }

    impl TempRepo {
        fn new(label: &str) -> Self {
            let serial = NEXT_TEMP_REPO.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "hoshino-git-{}-{}-{label}",
                std::process::id(),
                serial
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn init_repo(temp: &TempRepo) -> Repository {
        let mut options = RepositoryInitOptions::new();
        options.initial_head("main");
        let repo = Repository::init_opts(&temp.path, &options).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Hoshino Test").unwrap();
        config
            .set_str("user.email", "hoshino-test@example.invalid")
            .unwrap();
        repo
    }

    fn collect_at(path: &Path) -> (Option<Git>, Vec<Diagnostic>) {
        let mut diagnostics = Vec::new();
        let git = collect(path, &mut diagnostics);
        (git, diagnostics)
    }

    fn commit_file(repo: &Repository, relative: &str, contents: &str, message: &str) -> Oid {
        fs::write(repo.workdir().unwrap().join(relative), contents).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(relative)).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature = Signature::now("Hoshino Test", "hoshino-test@example.invalid").unwrap();
        let parent = repo
            .head()
            .ok()
            .and_then(|head| head.target())
            .map(|id| repo.find_commit(id).unwrap());
        let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parents,
        )
        .unwrap()
    }

    fn stage(repo: &Repository, relative: &str) {
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(relative)).unwrap();
        index.write().unwrap();
    }

    fn checkout_branch(repo: &Repository, name: &str) {
        let branch = repo.find_branch(name, BranchType::Local).unwrap();
        let commit = branch.get().peel_to_commit().unwrap();
        let mut options = CheckoutBuilder::new();
        options.force();
        repo.checkout_tree(commit.as_object(), Some(&mut options))
            .unwrap();
        repo.set_head(&format!("refs/heads/{name}")).unwrap();
    }

    #[test]
    fn no_repository_returns_none_without_diagnostic() {
        let temp = TempRepo::new("no-repo");
        let (git, diagnostics) = collect_at(&temp.path);
        assert!(git.is_none());
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn unborn_repository_is_reported_without_error() {
        let temp = TempRepo::new("unborn");
        let _repo = init_repo(&temp);
        let (git, diagnostics) = collect_at(&temp.path);
        let git = git.unwrap();
        assert_eq!(git.head, GitHead::Unborn);
        assert!(!git.dirty);
        assert_eq!((git.staged, git.unstaged, git.conflicts), (0, 0, 0));
        assert_eq!(git.upstream, None);
        assert_eq!(git.ahead, None);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn named_branch_clean_state_has_zero_counts() {
        let temp = TempRepo::new("clean");
        let repo = init_repo(&temp);
        commit_file(&repo, "tracked.txt", "base\n", "initial");
        let (git, diagnostics) = collect_at(&temp.path);
        let git = git.unwrap();
        assert_eq!(
            git.head,
            GitHead::Branch {
                name: "main".into()
            }
        );
        assert!(!git.dirty);
        assert_eq!((git.staged, git.unstaged, git.conflicts), (0, 0, 0));
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn staged_changes_are_counted_separately() {
        let temp = TempRepo::new("staged");
        let repo = init_repo(&temp);
        commit_file(&repo, "tracked.txt", "base\n", "initial");
        fs::write(temp.path.join("tracked.txt"), "staged\n").unwrap();
        stage(&repo, "tracked.txt");
        let (git, diagnostics) = collect_at(&temp.path);
        let git = git.unwrap();
        assert!(git.dirty);
        assert_eq!((git.staged, git.unstaged, git.conflicts), (1, 0, 0));
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn unstaged_changes_are_counted_separately() {
        let temp = TempRepo::new("unstaged");
        let repo = init_repo(&temp);
        commit_file(&repo, "tracked.txt", "base\n", "initial");
        fs::write(temp.path.join("tracked.txt"), "unstaged\n").unwrap();
        let (git, diagnostics) = collect_at(&temp.path);
        let git = git.unwrap();
        assert!(git.dirty);
        assert_eq!((git.staged, git.unstaged, git.conflicts), (0, 1, 0));
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn detached_head_keeps_commit_identity() {
        let temp = TempRepo::new("detached");
        let repo = init_repo(&temp);
        let commit = commit_file(&repo, "tracked.txt", "base\n", "initial");
        repo.set_head_detached(commit).unwrap();
        let (git, diagnostics) = collect_at(&temp.path);
        let git = git.unwrap();
        assert_eq!(
            git.head,
            GitHead::Detached {
                commit: commit.to_string()
            }
        );
        assert!(!git.dirty);
        assert_eq!(git.upstream, None);
        assert_eq!(git.ahead, None);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn branch_without_upstream_reports_no_ahead_value() {
        let temp = TempRepo::new("no-upstream");
        let repo = init_repo(&temp);
        commit_file(&repo, "tracked.txt", "base\n", "initial");
        let (git, diagnostics) = collect_at(&temp.path);
        let git = git.unwrap();
        assert_eq!(git.upstream, None);
        assert_eq!(git.ahead, None);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn local_branch_ahead_of_local_bare_upstream_is_counted() {
        let local = TempRepo::new("ahead-local");
        let remote = TempRepo::new("ahead-remote");
        let repo = init_repo(&local);
        let base = commit_file(&repo, "tracked.txt", "base\n", "initial");
        let _bare = Repository::init_bare(&remote.path).unwrap();
        let remote_url = remote.path.to_str().unwrap();
        repo.remote("origin", remote_url).unwrap();
        let mut origin = repo.find_remote("origin").unwrap();
        origin
            .push(&["refs/heads/main:refs/heads/main"], None)
            .unwrap();
        drop(origin);
        repo.reference(
            "refs/remotes/origin/main",
            base,
            true,
            "create local tracking ref for test",
        )
        .unwrap();
        let mut branch = repo.find_branch("main", BranchType::Local).unwrap();
        branch.set_upstream(Some("origin/main")).unwrap();
        drop(branch);
        commit_file(&repo, "tracked.txt", "ahead\n", "ahead");
        drop(repo);

        let (git, diagnostics) = collect_at(&local.path);
        let git = git.unwrap();
        assert_eq!(git.upstream.as_deref(), Some("origin/main"));
        assert_eq!(git.ahead, Some(1));
        assert!(!git.dirty);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn actual_merge_conflict_is_counted_without_panic() {
        let temp = TempRepo::new("conflict");
        let repo = init_repo(&temp);
        let base = commit_file(&repo, "conflict.txt", "base\n", "base");
        let base_commit = repo.find_commit(base).unwrap();
        repo.branch("theirs", &base_commit, false).unwrap();
        drop(base_commit);
        commit_file(&repo, "conflict.txt", "ours\n", "ours");
        checkout_branch(&repo, "theirs");
        let theirs = commit_file(&repo, "conflict.txt", "theirs\n", "theirs");
        checkout_branch(&repo, "main");

        let annotated = repo.find_annotated_commit(theirs).unwrap();
        let mut options = CheckoutBuilder::new();
        options.allow_conflicts(true).conflict_style_merge(true);
        repo.merge(&[&annotated], None, Some(&mut options)).unwrap();
        assert!(repo.index().unwrap().has_conflicts());
        drop(annotated);
        drop(repo);

        let (git, diagnostics) = collect_at(&temp.path);
        let git = git.unwrap();
        assert!(git.dirty);
        assert_eq!(git.conflicts, 1);
        assert_eq!((git.staged, git.unstaged), (0, 0));
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn invalid_index_is_reported_as_a_diagnostic() {
        let temp = TempRepo::new("invalid-index");
        let repo = init_repo(&temp);
        commit_file(&repo, "tracked.txt", "base\n", "initial");
        drop(repo);
        fs::write(temp.path.join(".git/index"), b"not an index").unwrap();

        let (git, diagnostics) = collect_at(&temp.path);
        assert!(git.is_none());
        assert!(diagnostics.iter().any(|item| item.code == "git-status"));
    }
}
