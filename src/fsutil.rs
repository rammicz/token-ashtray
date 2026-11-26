//! Replacing small files while other programs may have them open.
//!
//! A scanner, the search indexer, an editor or a provider CLI can hold a file
//! while it is replaced. Windows refuses the plain `MoveFileExW` replace that
//! `tempfile` uses as soon as the target has any open handle. `std::fs::rename`
//! falls back to a POSIX-semantics rename, which succeeds while every holder
//! shares delete access. A holder that does not share delete usually lets go
//! within moments, so those refusals are retried for a bounded time.
use std::{
    fs::{self, OpenOptions, Permissions},
    io::{self, Write},
    path::Path,
    time::Duration,
};

/// Total time spent waiting for a holder to let go.
const RETRY_BUDGET: Duration = Duration::from_secs(2);
const FIRST_BACKOFF: Duration = Duration::from_millis(10);
const MAX_BACKOFF: Duration = Duration::from_millis(200);

/// Replaces `path` with `bytes`, creating its directory if needed. Readers
/// and a crash see the old or the new file, never a partial or missing one.
/// `precheck` runs before every rename attempt; its error aborts the replace
/// and is returned as is. If the rename does not happen, the target is
/// unchanged and the temporary file is removed.
pub(crate) fn replace_atomically(
    path: &Path,
    bytes: &[u8],
    permissions: Option<Permissions>,
    mut precheck: impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    fs::create_dir_all(parent)?;
    // tempfile's own files carry FILE_ATTRIBUTE_TEMPORARY on Windows, which a
    // std rename would leave on the target, so open a plain file instead.
    let (mut file, mut temporary) = tempfile::Builder::new()
        .make_in(parent, |path| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            options.open(path)
        })?
        .into_parts();
    if let Some(permissions) = permissions {
        file.set_permissions(permissions)?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    retry_while_held(
        || {
            precheck()?;
            fs::rename(&temporary, path)
        },
        std::thread::sleep,
    )?;
    // The temporary path now names the target; dropping it must not delete it.
    temporary.disable_cleanup(true);
    // Make the rename durable, so a power cut cannot bring back the old file.
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

/// Runs `attempt` until it succeeds, fails for another reason, or the
/// waiting budget is spent.
fn retry_while_held(
    mut attempt: impl FnMut() -> io::Result<()>,
    mut sleep: impl FnMut(Duration),
) -> io::Result<()> {
    let mut budget = RETRY_BUDGET;
    let mut backoff = FIRST_BACKOFF;
    loop {
        match attempt() {
            Err(error) if held_by_another_handle(&error) && !budget.is_zero() => {
                let wait = backoff.min(budget);
                sleep(wait);
                budget -= wait;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
            result => return result,
        }
    }
}

/// ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION and ERROR_LOCK_VIOLATION.
/// A held target reports ERROR_ACCESS_DENIED, which an ACL or read-only
/// denial shares, so such a denial also waits out the budget before it is
/// returned. Matched by code because `ErrorKind::PermissionDenied` covers
/// even more. Elsewhere the same numbers mean unrelated errors that are
/// never retried.
pub(crate) fn held_by_another_handle(error: &io::Error) -> bool {
    cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

/// The names of the files in `directory`, for checking leftovers.
#[cfg(test)]
pub(crate) fn file_names(directory: &Path) -> Vec<String> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Feeds `results` to the retry loop and returns its result, the number
    /// of attempts and every wait.
    fn run(results: Vec<io::Result<()>>) -> (io::Result<()>, usize, Vec<Duration>) {
        let mut results = VecDeque::from(results);
        let mut attempts = 0;
        let mut waits = Vec::new();
        let result = retry_while_held(
            || {
                attempts += 1;
                results.pop_front().unwrap_or_else(|| Err(os(32)))
            },
            |wait| waits.push(wait),
        );
        (result, attempts, waits)
    }

    fn os(code: i32) -> io::Error {
        io::Error::from_raw_os_error(code)
    }

    #[test]
    fn sharing_violations_are_retried_until_the_holder_lets_go() {
        let (result, attempts, waits) = run(vec![Err(os(32)), Err(os(5)), Err(os(33)), Ok(())]);
        if cfg!(windows) {
            assert!(result.is_ok());
            assert_eq!(attempts, 4);
            assert_eq!(waits, [10, 20, 40].map(Duration::from_millis));
        } else {
            // 32 is EPIPE on Unix; only Windows sharing errors are retried.
            assert_eq!(result.unwrap_err().raw_os_error(), Some(32));
            assert_eq!(attempts, 1);
        }
    }

    #[test]
    fn other_errors_are_returned_without_retry() {
        for code in [2, 3, 183] {
            let (result, attempts, waits) = run(vec![Err(os(code)), Ok(())]);
            assert_eq!(result.unwrap_err().raw_os_error(), Some(code));
            assert_eq!(attempts, 1);
            assert!(waits.is_empty());
        }
        let precheck = io::Error::other("CLI login changed before replacement");
        let (result, attempts, _) = run(vec![Err(precheck), Ok(())]);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Other);
        assert_eq!(attempts, 1);
    }

    #[test]
    fn a_holder_that_never_lets_go_fails_within_the_budget() {
        let (result, attempts, waits) = run(vec![]);
        assert_eq!(result.unwrap_err().raw_os_error(), Some(32));
        if cfg!(windows) {
            assert_eq!(waits.iter().sum::<Duration>(), RETRY_BUDGET);
            assert!(waits.iter().all(|wait| *wait <= MAX_BACKOFF));
            assert_eq!(attempts, waits.len() + 1);
        } else {
            assert_eq!(attempts, 1);
        }
    }

    #[test]
    fn replace_writes_the_target_and_leaves_no_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        replace_atomically(&path, b"first", None, || Ok(())).unwrap();
        replace_atomically(&path, b"second", None, || Ok(())).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(file_names(directory.path()), ["state.json"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "new files stay private");
        }
    }

    #[test]
    fn failed_precheck_keeps_the_target_and_removes_the_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("auth.json");
        fs::write(&path, b"old").unwrap();
        let mut checks = 0;
        let error = replace_atomically(&path, b"new", None, || {
            checks += 1;
            Err(io::Error::other("changed"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "changed");
        assert_eq!(checks, 1);
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert_eq!(file_names(directory.path()), ["auth.json"]);
    }

    #[cfg(windows)]
    #[test]
    fn the_target_gets_no_temporary_attribute() {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x100;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        replace_atomically(&path, b"x", None, || Ok(())).unwrap();
        let attributes = fs::metadata(&path).unwrap().file_attributes();
        assert_eq!(attributes & FILE_ATTRIBUTE_TEMPORARY, 0);
    }
}
