//! Minimal PTY harness for the VT spike.
//!
//! Spawns a program on a pseudo-terminal, exposes resize and raw input, and
//! reads non-blocking output. macOS-only ioctl constants are defined locally
//! because the `libc` crate does not export them for Apple targets.

use std::io;
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TIOCSWINSZ: libc::c_ulong = 0x8008_7467;
const TIOCSCTTY: libc::c_ulong = 0x2000_7461;

pub struct Pty {
    master: RawFd,
    child: Option<Child>,
}

fn dup_fd(fd: RawFd) -> io::Result<RawFd> {
    let n = unsafe { libc::dup(fd) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n)
    }
}

fn poll_fd(fd: RawFd, events: libc::c_short, timeout_ms: i32) -> io::Result<bool> {
    let mut pfd = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    loop {
        let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if rc < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        return Ok(rc > 0 && (pfd.revents & events) != 0);
    }
}

impl Pty {
    pub fn spawn(argv: &[&str], rows: u16, cols: u16, cwd: &Path) -> io::Result<Pty> {
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        ws.ws_row = rows;
        ws.ws_col = cols;
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut ws,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }

        let stdin = unsafe { Stdio::from_raw_fd(dup_fd(slave)?) };
        let stdout = unsafe { Stdio::from_raw_fd(dup_fd(slave)?) };
        let stderr = unsafe { Stdio::from_raw_fd(dup_fd(slave)?) };
        unsafe {
            libc::close(slave);
        }

        let mut cmd = Command::new(argv[0]);
        cmd.args(&argv[1..])
            .current_dir(cwd)
            .env_clear()
            .env("TERM", "xterm-256color")
            .env("LANG", "en_US.UTF-8")
            .env("LC_ALL", "en_US.UTF-8")
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
            .stdin(stdin)
            .stdout(stdout)
            .stderr(stderr);
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                let _ = libc::ioctl(0, TIOCSCTTY, 0);
                Ok(())
            });
        }

        let child = cmd.spawn()?;
        let flags = unsafe { libc::fcntl(master, libc::F_GETFL) };
        if flags >= 0 {
            unsafe {
                libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
        // openpty does not set close-on-exec. Without this, every later pane
        // child inherits earlier masters and pty teardown can stall.
        unsafe {
            let fdflags = libc::fcntl(master, libc::F_GETFD);
            libc::fcntl(master, libc::F_SETFD, fdflags | libc::FD_CLOEXEC);
        }
        Ok(Pty {
            master,
            child: Some(child),
        })
    }

    pub fn resize(&self, rows: u16, cols: u16) {
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        ws.ws_row = rows;
        ws.ws_col = cols;
        unsafe {
            libc::ioctl(self.master, TIOCSWINSZ, &ws);
        }
    }

    /// Reads whatever is available within `timeout_ms`. `None` means idle.
    pub fn read(&mut self, timeout_ms: i32) -> io::Result<Option<Vec<u8>>> {
        if !poll_fd(self.master, libc::POLLIN, timeout_ms)? {
            return Ok(None);
        }
        let mut buf = [0u8; 65536];
        loop {
            let n = unsafe { libc::read(self.master, buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 {
                return Ok(Some(buf[..n as usize].to_vec()));
            }
            if n == 0 {
                return Ok(None);
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => return Ok(None),
                _ => return Err(e),
            }
        }
    }

    pub fn write_all(&mut self, bytes: &[u8], timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        let mut off = 0usize;
        while off < bytes.len() {
            let n = unsafe {
                libc::write(
                    self.master,
                    bytes[off..].as_ptr().cast(),
                    bytes.len() - off,
                )
            };
            if n > 0 {
                off += n as usize;
                continue;
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "pty write"));
                    }
                    let _ = poll_fd(self.master, libc::POLLOUT, 25)?;
                }
                _ => return Err(e),
            }
        }
        Ok(())
    }

    /// Returns `Some(exited)` without reaping the child twice.
    pub fn try_wait(&mut self) -> Option<bool> {
        let child = self.child.as_mut()?;
        match child.try_wait() {
            Ok(Some(_)) => Some(true),
            Ok(None) => Some(false),
            Err(_) => None,
        }
    }

    pub fn pid(&self) -> u32 {
        self.child.as_ref().map(|c| c.id()).unwrap_or(0)
    }

    /// Bounded teardown. Draining, closing the master, and reaping with a
    /// deadline keeps a macOS pty exit stall from hanging the harness.
    fn shutdown(&mut self) {
        for _ in 0..16 {
            let mut buf = [0u8; 8192];
            let n = unsafe { libc::read(self.master, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                break;
            }
        }
        unsafe {
            libc::close(self.master);
            self.master = -1;
        }
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.kill();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(2000) {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Never block the spike on a stuck exit; init reaps it when we exit.
        std::mem::forget(child);
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.shutdown();
    }
}
