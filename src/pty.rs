//! Unix pseudo-terminal ownership for the pane host.
//!
//! This module owns exactly one master descriptor and one child process: spawn,
//! non-blocking read, bounded write, resize and a bounded teardown. It holds no
//! pane state and writes no store; `pane_host` owns the slot state machine.
//!
//! The module is intentionally unreferenced by a command or surface until the
//! pane-surface card wires it. Keep the single allowance below module-wide and
//! remove it when the surface calls the host.
#![allow(dead_code)]

use std::fmt;
use std::fs::File;
use std::io;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Environment variables a pane is allowed to inherit. Everything else is
/// cleared so a caller's environment never reaches the shell.
pub const ENV_ALLOWLIST: [&str; 4] = ["HOME", "LANG", "LC_ALL", "TERM"];

/// Fixed environment values applied inside every pane.
pub const FIXED_ENV: [(&str, &str); 4] = [
    ("TERM", "xterm-256color"),
    ("LANG", "en_US.UTF-8"),
    ("LC_ALL", "en_US.UTF-8"),
    ("PATH", "/usr/bin:/bin:/usr/sbin:/sbin"),
];

/// Maximum time a single PTY write may make no progress.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum PtyError {
    /// The program could not be created.
    Spawn(String),
    /// An OS call failed while the child was alive.
    Io(String),
    /// A bounded write made no progress before its deadline.
    WriteTimeout,
    /// The master descriptor was already closed.
    Closed,
}

impl fmt::Display for PtyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(detail) => write!(f, "PTY spawn failed: {detail}"),
            Self::Io(detail) => write!(f, "PTY I/O failed: {detail}"),
            Self::WriteTimeout => f.write_str("PTY write timed out"),
            Self::Closed => f.write_str("PTY master is closed"),
        }
    }
}

#[derive(Debug)]
pub enum StopError {
    /// The child did not exit before the deadline.
    Timeout,
    /// The master or child could not be handled.
    Io(String),
}

impl fmt::Display for StopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("PTY teardown timed out"),
            Self::Io(detail) => write!(f, "PTY teardown failed: {detail}"),
        }
    }
}

/// One spawned pseudo-terminal.
pub struct PtyProcess {
    master: RawFd,
    child: Child,
    exited: Option<i32>,
}

impl PtyProcess {
    /// Spawns `program` with an argv array (never a shell string), a fixed cwd,
    /// an allowlist-only environment, and the given initial window size.
    pub fn spawn(
        program: &Path,
        args: &[&str],
        cwd: &Path,
        env: &[(String, String)],
        rows: u16,
        cols: u16,
    ) -> Result<Self, PtyError> {
        #[cfg(test)]
        record_spawn_attempt();

        if rows == 0 || cols == 0 {
            return Err(PtyError::Spawn("window size must be positive".to_string()));
        }

        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        let winsize = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &winsize as *const libc::winsize as *mut libc::winsize,
            )
        };
        if opened != 0 {
            return Err(PtyError::Spawn(format!(
                "openpty failed: {}",
                io::Error::last_os_error()
            )));
        }

        if let Err(error) = set_fd_flags(master, libc::FD_CLOEXEC, true) {
            close_fd(master);
            close_fd(slave);
            return Err(PtyError::Spawn(error.to_string()));
        }
        if let Err(error) = set_nonblocking(master, true) {
            close_fd(master);
            close_fd(slave);
            return Err(PtyError::Spawn(error.to_string()));
        }

        let built = (|| -> io::Result<Child> {
            // Taking ownership of the slave descriptor means every error path
            // closes it exactly once when this closure returns.
            let slave_file = unsafe { File::from_raw_fd(slave) };
            let stdin = slave_file.try_clone()?;
            let stdout = slave_file.try_clone()?;
            let stderr = slave_file.try_clone()?;

            let mut command = Command::new(program);
            command
                .args(args)
                .current_dir(cwd)
                .env_clear()
                .stdin(Stdio::from(stdin))
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr));
            for (name, value) in env {
                command.env(name, value);
            }
            // The child must lead a new session and adopt the slave as its
            // controlling terminal. Only async-signal-safe calls belong here.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            command.spawn()
        })();

        // The closure above owned and closed the parent's slave descriptor;
        // otherwise the master never reports EOF after the child exits.
        let child = match built {
            Ok(child) => child,
            Err(error) => {
                close_fd(master);
                return Err(PtyError::Spawn(error.to_string()));
            }
        };

        Ok(Self {
            master,
            child,
            exited: None,
        })
    }

    /// Reads currently available bytes. `Ok(None)` means "would block",
    /// `Ok(Some(0))` means end of file. A closed slave is reported as EOF.
    pub fn read(&mut self, buffer: &mut [u8]) -> Result<Option<usize>, PtyError> {
        if self.master < 0 {
            return Err(PtyError::Closed);
        }
        let read = unsafe {
            libc::read(
                self.master,
                buffer.as_mut_ptr().cast::<libc::c_void>(),
                buffer.len(),
            )
        };
        if read > 0 {
            return Ok(Some(read as usize));
        }
        if read == 0 {
            return Ok(Some(0));
        }
        let error = io::Error::last_os_error();
        match error.kind() {
            io::ErrorKind::WouldBlock => Ok(None),
            io::ErrorKind::Interrupted => Ok(None),
            // macOS reports EIO on a master whose slave side is gone.
            _ if error.raw_os_error() == Some(libc::EIO) => Ok(Some(0)),
            _ => Err(PtyError::Io(error.to_string())),
        }
    }

    /// Writes every byte or fails at the deadline. Never spins unbounded.
    ///
    /// A zero deadline is the deterministic bound seam: it reports
    /// [`PtyError::WriteTimeout`] without attempting a write.
    pub fn write(&mut self, bytes: &[u8], timeout: Duration) -> Result<usize, PtyError> {
        if self.master < 0 {
            return Err(PtyError::Closed);
        }
        if bytes.is_empty() {
            return Ok(0);
        }
        if timeout.is_zero() {
            return Err(PtyError::WriteTimeout);
        }
        let deadline = Instant::now() + timeout;
        let mut written = 0usize;
        while written < bytes.len() {
            let chunk = &bytes[written..];
            let result = unsafe {
                libc::write(
                    self.master,
                    chunk.as_ptr().cast::<libc::c_void>(),
                    chunk.len(),
                )
            };
            if result > 0 {
                written += result as usize;
                continue;
            }
            if result < 0 {
                let error = io::Error::last_os_error();
                match error.kind() {
                    io::ErrorKind::WouldBlock => {
                        if !wait_writable(self.master, deadline) {
                            return Err(PtyError::WriteTimeout);
                        }
                        continue;
                    }
                    io::ErrorKind::Interrupted => continue,
                    _ => return Err(PtyError::Io(error.to_string())),
                }
            }
            return Err(PtyError::Io("write returned zero".to_string()));
        }
        Ok(written)
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<(), PtyError> {
        if self.master < 0 {
            return Err(PtyError::Closed);
        }
        let winsize = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let result = unsafe {
            libc::ioctl(
                self.master,
                libc::TIOCSWINSZ as libc::c_ulong,
                &winsize as *const libc::winsize,
            )
        };
        if result == -1 {
            return Err(PtyError::Io(io::Error::last_os_error().to_string()));
        }
        Ok(())
    }

    /// Non-blocking exit check. A status is cached so it is observed once and
    /// repeated calls never wait.
    pub fn poll_exit(&mut self) -> Result<Option<i32>, PtyError> {
        if let Some(code) = self.exited {
            return Ok(Some(code));
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                let code = status.code().unwrap_or(-1);
                self.exited = Some(code);
                Ok(Some(code))
            }
            Ok(None) => Ok(None),
            Err(error) => Err(PtyError::Io(error.to_string())),
        }
    }

    /// EOF is only a hint; the exit status is checked separately.
    pub fn is_exited(&self) -> bool {
        self.exited.is_some()
    }

    /// Drains available output, closes the master, kills the child, and reaps it
    /// before `deadline`. A zero deadline never reaps and reports a timeout, so
    /// an unreapable child can be modeled deterministically in tests.
    pub fn stop(&mut self, deadline: Duration) -> Result<i32, StopError> {
        self.drain_bounded();
        self.close_master();

        if let Some(code) = self.exited {
            return Ok(code);
        }
        let pid = self.child.id() as libc::pid_t;
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        // A zero deadline is the deterministic state-level timeout seam: the
        // caller has already decided to model an unreaped child.
        if deadline.is_zero() {
            return Err(StopError::Timeout);
        }

        let end = Instant::now() + deadline;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    let code = status.code().unwrap_or(-1);
                    self.exited = Some(code);
                    return Ok(code);
                }
                Ok(None) => {}
                Err(error) => return Err(StopError::Io(error.to_string())),
            }
            if Instant::now() >= end {
                return Err(StopError::Timeout);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Reads until the buffer is empty or a small bound elapses. Used before
    /// teardown so buffered output reaches the grid, but never unbounded.
    fn drain_bounded(&mut self) {
        if self.master < 0 {
            return;
        }
        let deadline = Instant::now() + Duration::from_millis(100);
        let mut buffer = [0u8; 8192];
        while Instant::now() < deadline {
            match self.read(&mut buffer) {
                Ok(Some(0)) | Err(_) => break,
                Ok(Some(_)) => continue,
                Ok(None) => {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }

    fn close_master(&mut self) {
        if self.master >= 0 {
            close_fd(self.master);
            self.master = -1;
        }
    }

    #[cfg(test)]
    pub fn master_flags(&self) -> i32 {
        unsafe { libc::fcntl(self.master, libc::F_GETFD) }
    }

    #[cfg(test)]
    pub fn master_fd(&self) -> RawFd {
        self.master
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        if self.exited.is_none() {
            let pid = self.child.id() as libc::pid_t;
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
            let _ = self.child.try_wait();
        }
        self.close_master();
    }
}

impl AsRawFd for PtyProcess {
    fn as_raw_fd(&self) -> RawFd {
        self.master
    }
}

fn wait_writable(fd: RawFd, deadline: Instant) -> bool {
    let now = Instant::now();
    if now >= deadline {
        return false;
    }
    let remaining = (deadline - now).as_millis().min(50) as libc::c_int;
    let mut poll_fd = libc::pollfd {
        fd,
        events: libc::POLLOUT,
        revents: 0,
    };
    loop {
        let ready = unsafe { libc::poll(&mut poll_fd, 1, remaining) };
        if ready > 0 {
            return true;
        }
        if ready < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Instant::now() < deadline && ready != 0;
    }
}

fn close_fd(fd: libc::c_int) {
    unsafe {
        libc::close(fd);
    }
}

fn set_fd_flags(fd: libc::c_int, flag: libc::c_int, enable: bool) -> io::Result<()> {
    let current = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if current < 0 {
        return Err(io::Error::last_os_error());
    }
    let next = if enable {
        current | flag
    } else {
        current & !flag
    };
    if unsafe { libc::fcntl(fd, libc::F_SETFD, next) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_nonblocking(fd: libc::c_int, enable: bool) -> io::Result<()> {
    let current = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if current < 0 {
        return Err(io::Error::last_os_error());
    }
    let next = if enable {
        current | libc::O_NONBLOCK
    } else {
        current & !libc::O_NONBLOCK
    };
    if unsafe { libc::fcntl(fd, libc::F_SETFL, next) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Builds the fixed environment used by every pane. `HOME` is inherited only
/// when present; it is the sole caller-provided value.
pub fn pane_env() -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = FIXED_ENV
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    if let Ok(home) = std::env::var("HOME") {
        env.push(("HOME".to_string(), home));
    }
    env
}

/// True when `program` exists as an absolute executable path.
pub fn program_exists(program: &Path) -> bool {
    program.is_absolute() && program.is_file()
}

// Per-thread so parallel tests never see another test's spawns.
#[cfg(test)]
std::thread_local! {
    static SPAWN_ATTEMPTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_spawn_attempt() {
    SPAWN_ATTEMPTS.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
pub fn spawn_attempts() -> usize {
    SPAWN_ATTEMPTS.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("iskra-pty-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn spawn_shell(cwd: &Path) -> PtyProcess {
        PtyProcess::spawn(
            Path::new("/bin/bash"),
            &["--norc", "--noprofile"],
            cwd,
            &pane_env(),
            24,
            80,
        )
        .unwrap()
    }

    fn read_until(pty: &mut PtyProcess, needle: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        let mut collected = Vec::new();
        let mut buffer = [0u8; 4096];
        while Instant::now() < deadline {
            match pty.read(&mut buffer).unwrap() {
                Some(0) => break,
                Some(n) => {
                    collected.extend_from_slice(&buffer[..n]);
                    if String::from_utf8_lossy(&collected).contains(needle) {
                        break;
                    }
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        String::from_utf8_lossy(&collected).to_string()
    }

    fn wait_exit(pty: &mut PtyProcess, timeout: Duration) -> Option<i32> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(code) = pty.poll_exit().unwrap() {
                return Some(code);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn master_fd_is_close_on_exec() {
        let dir = temp_dir("cloexec");
        let pty = spawn_shell(&dir);
        assert!(
            pty.master_flags() & libc::FD_CLOEXEC != 0,
            "master must not leak into the child"
        );
        assert!(pty.master_fd() >= 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_program_fails_without_a_child() {
        let dir = temp_dir("invalid");
        let result = PtyProcess::spawn(
            Path::new("/nonexistent/iskra-pty"),
            &[],
            &dir,
            &pane_env(),
            24,
            80,
        );
        assert!(result.is_err(), "a missing program must not spawn");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_deadline_is_enforced() {
        let dir = temp_dir("write-deadline");
        let mut pty =
            PtyProcess::spawn(Path::new("/bin/sleep"), &["30"], &dir, &pane_env(), 24, 80).unwrap();

        let started = Instant::now();
        let result = pty.write(b"x", Duration::ZERO);
        assert!(
            matches!(result, Err(PtyError::WriteTimeout)),
            "a zero deadline must report a timeout: {result:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the deadline check must not wait"
        );

        // A large accepted write still completes or times out bounded.
        let payload = vec![b'x'; 1024 * 1024];
        let started = Instant::now();
        let result = pty.write(&payload, Duration::from_millis(150));
        assert!(
            matches!(&result, Ok(written) if *written == payload.len())
                || matches!(result, Err(PtyError::WriteTimeout)),
            "a large write must be bounded: {result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        let _ = pty.stop(Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn eof_and_exit_are_detected_once() {
        let dir = temp_dir("eof");
        let mut pty = PtyProcess::spawn(
            Path::new("/bin/sh"),
            &["-c", "printf 'BYE\\n'; exit 3"],
            &dir,
            &pane_env(),
            24,
            80,
        )
        .unwrap();

        let output = read_until(&mut pty, "BYE", Duration::from_secs(3));
        assert!(output.contains("BYE"), "{output:?}");
        let code = wait_exit(&mut pty, Duration::from_secs(3));
        assert_eq!(code, Some(3));

        // A cached status is reported again without waiting.
        assert_eq!(pty.poll_exit().unwrap(), Some(3));
        assert!(pty.is_exited());

        let mut buffer = [0u8; 64];
        let mut saw_eof = false;
        for _ in 0..50 {
            match pty.read(&mut buffer) {
                Ok(Some(0)) => {
                    saw_eof = true;
                    break;
                }
                Ok(_) => continue,
                Err(PtyError::Closed) => {
                    saw_eof = true;
                    break;
                }
                Err(error) => panic!("unexpected read error: {error}"),
            }
        }
        assert!(saw_eof, "the master must report EOF after the child exits");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resize_is_reported_to_the_child() {
        let dir = temp_dir("resize");
        let mut pty = spawn_shell(&dir);
        pty.resize(30, 100).unwrap();
        pty.write(b"stty size\r", Duration::from_secs(2)).unwrap();
        let output = read_until(&mut pty, "30 100", Duration::from_secs(3));
        assert!(output.contains("30 100"), "{output:?}");
        let _ = pty.stop(Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn environment_allowlist_excludes_inherited_values() {
        let dir = temp_dir("env");
        unsafe {
            std::env::set_var("ISKRA_PTY_CANARY", "leaked");
        }
        let mut pty = spawn_shell(&dir);
        pty.write(
            b"printf 'CANARY=[%s]\\n' \"$ISKRA_PTY_CANARY\"\r",
            Duration::from_secs(2),
        )
        .unwrap();
        let output = read_until(&mut pty, "CANARY=[]", Duration::from_secs(3));
        assert!(output.contains("CANARY=[]"), "{output:?}");
        unsafe {
            std::env::remove_var("ISKRA_PTY_CANARY");
        }
        let _ = pty.stop(Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
