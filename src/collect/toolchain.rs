use super::project::{Detection, Language};
use crate::{
    limits::Limits,
    model::{Diagnostic, DiagnosticSeverity, Toolchain},
};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Eq, PartialEq)]
struct Runtime {
    bin: &'static str,
    args: &'static [&'static str],
}

pub fn collect(
    detection: &Detection,
    full: bool,
    limits: &Limits,
    diags: &mut Vec<Diagnostic>,
) -> Vec<Toolchain> {
    collect_with_runner(detection, full, limits, diags, |bin, args, limits| {
        run_path(Path::new(bin), args, limits)
    })
}

fn collect_with_runner(
    detection: &Detection,
    full: bool,
    limits: &Limits,
    diags: &mut Vec<Diagnostic>,
    runner: impl Fn(&str, &[&str], &Limits) -> Result<String, String>,
) -> Vec<Toolchain> {
    let languages = if full {
        detection.detected.clone()
    } else {
        detection.primary.into_iter().collect()
    };
    let mut probes = Vec::<(Runtime, Result<(String, String), String>)>::new();
    languages
        .into_iter()
        .map(|language| {
            let candidates = runtimes(language, detection.bun);
            let result = if let Some((_, result)) =
                probes.iter().find(|(runtime, _)| *runtime == candidates[0])
            {
                result.clone()
            } else {
                let result = probe_with_runner(candidates, limits, &runner);
                probes.push((candidates[0], result.clone()));
                result
            };
            match result {
                Ok((runtime, version)) => Toolchain {
                    language: language.name().into(),
                    runtime,
                    version: Some(version),
                },
                Err(message) => {
                    diags.push(diag("toolchain", &message, candidates[0].bin));
                    Toolchain {
                        language: language.name().into(),
                        runtime: candidates[0].bin.into(),
                        version: None,
                    }
                }
            }
        })
        .collect()
}

fn runtimes(language: Language, bun: bool) -> &'static [Runtime] {
    const RUST: &[Runtime] = &[Runtime {
        bin: "rustc",
        args: &["--version"],
    }];
    const PYTHON: &[Runtime] = &[
        Runtime {
            bin: "python3",
            args: &["--version"],
        },
        Runtime {
            bin: "python",
            args: &["--version"],
        },
    ];
    const NODE: &[Runtime] = &[Runtime {
        bin: "node",
        args: &["--version"],
    }];
    const BUN: &[Runtime] = &[Runtime {
        bin: "bun",
        args: &["--version"],
    }];
    const GO: &[Runtime] = &[Runtime {
        bin: "go",
        args: &["version"],
    }];
    match language {
        Language::Rust => RUST,
        Language::Python => PYTHON,
        Language::TypeScript | Language::JavaScript if bun => BUN,
        Language::TypeScript | Language::JavaScript => NODE,
        Language::Go => GO,
    }
}

fn probe_with_runner(
    candidates: &[Runtime],
    limits: &Limits,
    runner: &impl Fn(&str, &[&str], &Limits) -> Result<String, String>,
) -> Result<(String, String), String> {
    let mut failures = Vec::new();
    for runtime in candidates {
        match runner(runtime.bin, runtime.args, limits) {
            Ok(version) => return Ok((runtime.bin.into(), version)),
            Err(error) => failures.push(format!("{}: {error}", runtime.bin)),
        }
    }
    Err(failures.join("; "))
}

struct Output {
    bytes: Vec<u8>,
    truncated: bool,
}

fn run_path(path: &Path, args: &[&str], limits: &Limits) -> Result<String, String> {
    let mut command = Command::new(path);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdout = child.stdout.take().ok_or("stdout pipe unavailable")?;
    let stderr = child.stderr.take().ok_or("stderr pipe unavailable")?;
    let max = limits.subprocess_output_bytes as usize;
    let stdout_reader = thread::spawn(move || read_bounded(stdout, max));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, max));
    let deadline = Instant::now() + Duration::from_millis(limits.subprocess_timeout_ms);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_readers(stdout_reader, stderr_reader);
                return Err("timed out".into());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_readers(stdout_reader, stderr_reader);
                return Err(error.to_string());
            }
        }
    };
    let (stdout, stderr) = join_readers(stdout_reader, stderr_reader)?;
    if stdout.truncated || stderr.truncated {
        return Err(format!("output truncated at {max} bytes"));
    }
    let stdout = String::from_utf8(stdout.bytes).map_err(|_| "non UTF-8 stdout".to_string())?;
    let stderr = String::from_utf8(stderr.bytes).map_err(|_| "non UTF-8 stderr".to_string())?;
    if !status.success() {
        return Err(format!("exited {status}: {}", stderr.trim()));
    }
    Ok(if stdout.trim().is_empty() {
        stderr.trim()
    } else {
        stdout.trim()
    }
    .into())
}

fn join_readers(
    stdout: thread::JoinHandle<Result<Output, String>>,
    stderr: thread::JoinHandle<Result<Output, String>>,
) -> Result<(Output, Output), String> {
    let stdout = stdout
        .join()
        .map_err(|_| "stdout reader failed".to_string())??;
    let stderr = stderr
        .join()
        .map_err(|_| "stderr reader failed".to_string())??;
    Ok((stdout, stderr))
}

fn read_bounded(mut reader: impl Read, max: usize) -> Result<Output, String> {
    let mut bytes = Vec::with_capacity(max.min(8192));
    let mut chunk = [0; 8192];
    loop {
        let remaining = max.saturating_sub(bytes.len());
        let read_len = if remaining == 0 {
            1
        } else {
            chunk.len().min(remaining + 1)
        };
        let count = reader
            .read(&mut chunk[..read_len])
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Ok(Output {
                bytes,
                truncated: false,
            });
        }
        if count > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            return Ok(Output {
                bytes,
                truncated: true,
            });
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
}

fn diag(code: &str, message: &str, subject: &str) -> Diagnostic {
    Diagnostic {
        severity: DiagnosticSeverity::Warning,
        code: code.into(),
        message: message.into(),
        subject: Some(subject.into()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "hoshino-toolchain-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn fake(directory: &std::path::Path, name: &str, body: &str) {
        let path = directory.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn limits() -> Limits {
        Limits {
            subprocess_timeout_ms: 5_000,
            subprocess_output_bytes: 256,
            ..Limits::default()
        }
    }

    #[test]
    fn probes_success_missing_nonzero_invalid_and_metacharacters_without_a_shell() {
        let directory = directory();
        assert!(run_path(&directory.join("missing"), &["--version"], &limits()).is_err());
        let spaced_directory = directory.join("path with spaces");
        fs::create_dir_all(&spaced_directory).unwrap();
        fake(&spaced_directory, "good", "printf 'good %s' \"$1\"");
        assert_eq!(
            run_path(&spaced_directory.join("good"), &["--version"], &limits()).unwrap(),
            "good --version"
        );
        fake(&directory, "bad", "echo nope >&2; exit 3");
        let error = run_path(&directory.join("bad"), &["--version"], &limits()).unwrap_err();
        assert!(error.contains("exited"), "unexpected error: {error}");
        fake(&directory, "invalid", "printf '\\377'");
        let error = run_path(&directory.join("invalid"), &["--version"], &limits()).unwrap_err();
        assert!(error.contains("non UTF-8"), "unexpected error: {error}");
        fake(&directory, "argv", "printf '%s' \"$1\"");
        let marker = directory.join("not-created");
        let argument = format!("--version; touch {}", marker.display());
        assert_eq!(
            run_path(&directory.join("argv"), &[&argument], &limits()).unwrap(),
            argument
        );
        assert!(!marker.exists());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn python_falls_back_and_full_mode_deduplicates_runtime_argv() {
        let directory = directory();
        fake(&directory, "python", "printf 'Python fallback'");
        let detection = Detection {
            primary: Some(Language::Python),
            detected: vec![Language::Python],
            bun: false,
        };
        let mut diagnostics = Vec::new();
        let toolchains = collect_with_runner(
            &detection,
            false,
            &limits(),
            &mut diagnostics,
            |bin, args, limits| run_path(&directory.join(bin), args, limits),
        );
        assert_eq!(toolchains[0].runtime, "python");
        assert!(diagnostics.is_empty());

        let count = directory.join("count");
        fake(
            &directory,
            "node",
            &format!("echo x >> '{}'; printf node", count.display()),
        );
        let detection = Detection {
            primary: Some(Language::TypeScript),
            detected: vec![Language::TypeScript, Language::JavaScript],
            bun: false,
        };
        assert_eq!(
            collect_with_runner(
                &detection,
                true,
                &limits(),
                &mut diagnostics,
                |bin, args, limits| run_path(&directory.join(bin), args, limits),
            )
            .len(),
            2
        );
        assert_eq!(fs::read_to_string(count).unwrap().lines().count(), 1);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn flood_and_hang_are_diagnostics_and_timed_child_is_gone() {
        let directory = directory();
        fake(
            &directory,
            "flood",
            "printf 123456789012345678901234567890123",
        );
        let flood_limits = Limits {
            subprocess_output_bytes: 32,
            ..limits()
        };
        let error = run_path(&directory.join("flood"), &["--version"], &flood_limits).unwrap_err();
        assert!(error.contains("truncated"), "unexpected error: {error}");
        let pid = directory.join("pid");
        fake(
            &directory,
            "hang",
            &format!("echo $$ > '{}'; while :; do :; done", pid.display()),
        );
        let hang_limits = Limits {
            // Give the script time to publish its PID before the bounded
            // probe terminates it; keep the test bound finite while allowing
            // concurrent fake executable startup.
            subprocess_timeout_ms: 5_000,
            ..limits()
        };
        assert!(
            run_path(&directory.join("hang"), &["--version"], &hang_limits)
                .unwrap_err()
                .contains("timed out")
        );
        let child = fs::read_to_string(pid).unwrap();
        assert!(
            !Command::new("kill")
                .args(["-0", child.trim()])
                .status()
                .unwrap()
                .success()
        );
        let _ = fs::remove_dir_all(directory);
    }
}
