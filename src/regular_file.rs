use std::{fmt, fs::File, io, path::Path};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SymlinkPolicy {
    Follow,
    NoFollow,
}

#[derive(Debug)]
pub(crate) enum OpenRegularError {
    Io(io::Error),
    NotRegular,
    #[cfg(not(any(unix, windows)))]
    NonblockingUnavailable,
}

impl fmt::Display for OpenRegularError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::NotRegular => f.write_str("path is not a regular file"),
            #[cfg(not(any(unix, windows)))]
            Self::NonblockingUnavailable => {
                f.write_str("platform cannot safely open arbitrary paths without blocking")
            }
        }
    }
}

impl std::error::Error for OpenRegularError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            #[cfg(not(any(unix, windows)))]
            Self::NotRegular | Self::NonblockingUnavailable => None,
            #[cfg(any(unix, windows))]
            Self::NotRegular => None,
        }
    }
}

pub(crate) fn open_regular(path: &Path, policy: SymlinkPolicy) -> Result<File, OpenRegularError> {
    open_regular_with(path, policy, || {})
}

#[cfg(unix)]
fn open_regular_with<F>(
    path: &Path,
    policy: SymlinkPolicy,
    before_open: F,
) -> Result<File, OpenRegularError>
where
    F: FnOnce(),
{
    use std::os::unix::fs::OpenOptionsExt;

    let mut flags = libc::O_NONBLOCK | libc::O_CLOEXEC;
    if policy == SymlinkPolicy::NoFollow {
        flags |= libc::O_NOFOLLOW;
    }
    let mut options = File::options();
    options.read(true).custom_flags(flags);
    before_open();
    let file = options.open(path).map_err(OpenRegularError::Io)?;
    if !file.metadata().map_err(OpenRegularError::Io)?.is_file() {
        return Err(OpenRegularError::NotRegular);
    }
    Ok(file)
}

#[cfg(windows)]
fn open_regular_with<F>(
    path: &Path,
    policy: SymlinkPolicy,
    before_open: F,
) -> Result<File, OpenRegularError>
where
    F: FnOnce(),
{
    use std::os::windows::fs::OpenOptionsExt;

    // CreateFile opens the reparse point itself rather than resolving it.
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    let mut options = File::options();
    options.read(true);
    if policy == SymlinkPolicy::NoFollow {
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    before_open();
    let file = options.open(path).map_err(OpenRegularError::Io)?;
    if !file.metadata().map_err(OpenRegularError::Io)?.is_file() {
        return Err(OpenRegularError::NotRegular);
    }
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn open_regular_with<F>(_: &Path, _: SymlinkPolicy, _: F) -> Result<File, OpenRegularError>
where
    F: FnOnce(),
{
    Err(OpenRegularError::NonblockingUnavailable)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs::{remove_file, write},
        os::unix::fs::symlink,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    fn fixture(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "hoshino-regular-file-{name}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn swap_to_fifo(path: &Path) {
        remove_file(path).unwrap();
        assert!(Command::new("mkfifo").arg(path).status().unwrap().success());
    }

    #[test]
    fn rejects_fifo_swapped_immediately_before_follow_open() {
        let path = fixture("follow-fifo");
        write(&path, "regular").unwrap();
        assert!(matches!(
            open_regular_with(&path, SymlinkPolicy::Follow, || swap_to_fifo(&path)),
            Err(OpenRegularError::NotRegular)
        ));
        remove_file(path).unwrap();
    }

    #[test]
    fn rejects_fifo_swapped_immediately_before_no_follow_open() {
        let path = fixture("no-follow-fifo");
        write(&path, "regular").unwrap();
        assert!(matches!(
            open_regular_with(&path, SymlinkPolicy::NoFollow, || swap_to_fifo(&path)),
            Err(OpenRegularError::NotRegular)
        ));
        remove_file(path).unwrap();
    }

    #[test]
    fn no_follow_rejects_a_symlink_swapped_immediately_before_open() {
        let path = fixture("no-follow-link");
        let target = fixture("target");
        write(&path, "regular").unwrap();
        write(&target, "regular").unwrap();
        assert!(matches!(
            open_regular_with(&path, SymlinkPolicy::NoFollow, || {
                remove_file(&path).unwrap();
                symlink(&target, &path).unwrap();
            }),
            Err(OpenRegularError::Io(_))
        ));
        remove_file(path).unwrap();
        remove_file(target).unwrap();
    }

    #[test]
    fn follow_opens_regular_symlinks() {
        let path = fixture("follow-link");
        let target = fixture("target");
        write(&target, "regular").unwrap();
        symlink(&target, &path).unwrap();
        assert!(open_regular(&path, SymlinkPolicy::Follow).is_ok());
        remove_file(path).unwrap();
        remove_file(target).unwrap();
    }
}
