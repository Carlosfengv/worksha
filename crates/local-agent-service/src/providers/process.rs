//! Bounded output for local version and process inspection, on a blocking worker.
use std::{
    io::{ErrorKind, Read},
    os::fd::AsRawFd,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 256 * 1024;

pub(super) fn output(binary: &Path, args: &[&str]) -> Option<String> {
    output_with_budget(binary, args, Duration::from_millis(1200))
}

fn output_with_budget(binary: &Path, args: &[&str], budget: Duration) -> Option<String> {
    let deadline = Instant::now() + budget;
    let mut child = Command::new(binary)
        .args(args)
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let result = collect(&mut child, deadline);
    if result.is_none() {
        // Only this directly spawned child is owned here. Never signal a
        // discovered process or a process group inferred from an Agent name.
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}

fn collect(child: &mut Child, deadline: Instant) -> Option<String> {
    let mut stdout = child.stdout.take()?;
    let fd = stdout.as_raw_fd();
    // SAFETY: stdout owns this live descriptor throughout both calls; no other
    // thread reads it. Preserve the existing file status flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return None;
    }
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    let mut eof = false;
    let mut status = None;
    loop {
        if Instant::now() >= deadline {
            return None;
        }
        let mut progressed = false;
        if !eof {
            match stdout.read(&mut buffer) {
                Ok(0) => eof = true,
                Ok(length) => {
                    if length > OUTPUT_LIMIT.saturating_sub(bytes.len()) {
                        return None;
                    }
                    bytes.extend_from_slice(&buffer[..length]);
                    progressed = true;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return None,
            }
        }
        if status.is_none() {
            status = child.try_wait().ok()?;
        }
        if let Some(status) = status {
            if !status.success() {
                return None;
            }
            if eof {
                return String::from_utf8(bytes).ok();
            }
        }
        if !progressed {
            std::thread::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_utf8_output_is_complete() {
        assert_eq!(
            output(Path::new("/bin/sh"), &["-c", "printf 'fixture 0.0.0\\n'"]),
            Some("fixture 0.0.0\n".into())
        );
    }

    #[test]
    fn failed_process_or_invalid_utf8_does_not_become_a_verified_version() {
        for script in ["printf 'fixture'; exit 7", "printf '\\377'"] {
            assert!(output(Path::new("/bin/sh"), &["-c", script]).is_none());
        }
        assert!(output(Path::new("/nonexistent/local-agent-review-fixture"), &[]).is_none());
    }

    #[test]
    fn exact_output_limit_is_accepted_but_overflow_is_rejected() {
        let complete = output(Path::new("/usr/bin/head"), &["-c", "262144", "/dev/zero"]).unwrap();
        assert_eq!(complete.len(), OUTPUT_LIMIT);
        assert!(output(Path::new("/usr/bin/head"), &["-c", "262145", "/dev/zero"],).is_none());
    }

    #[test]
    fn exited_parent_cannot_leave_the_reader_waiting_for_inherited_stdout() {
        let started = Instant::now();
        assert!(
            output_with_budget(
                Path::new("/bin/sh"),
                &["-c", "/bin/sleep 0.4 & printf 'fixture'; exit 0"],
                Duration::from_millis(100),
            )
            .is_none()
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        // This fixture's unowned descendant exits naturally within 400 ms;
        // no reader thread or open pipe is retained by the probe.
    }

    #[test]
    fn closing_stdout_does_not_hide_a_child_that_is_still_running() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("pid");
        let script = "printf '%s' $$ > \"$1\"; exec 1>&-; exec /bin/sleep 2";
        assert!(
            output_with_budget(
                Path::new("/bin/sh"),
                &["-c", script, "probe", pid_file.to_str().unwrap()],
                Duration::from_millis(100),
            )
            .is_none()
        );
        let pid: libc::pid_t = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
        // SAFETY: signal zero only checks the fixture PID; it sends no signal.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[test]
    fn repeated_timeouts_do_not_prevent_the_next_successful_probe() {
        for _ in 0..4 {
            assert!(
                output_with_budget(
                    Path::new("/bin/sh"),
                    &["-c", "exec /bin/sleep 2"],
                    Duration::from_millis(40),
                )
                .is_none()
            );
        }
        assert_eq!(
            output(Path::new("/bin/sh"), &["-c", "printf 'ready'"]),
            Some("ready".into())
        );
    }
}
