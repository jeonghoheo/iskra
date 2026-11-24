//! Process construction for `run_command` (directive v1.1-A, shell hardening).
//!
//! Everything a spawned program can see is decided here, by Iskra, never by the
//! model: the pinned absolute program path, a cleared environment with an explicit
//! allowlist, the forced git isolation options, a private process group, and (on
//! macOS) the Seatbelt profile the program runs under.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::process::Command;

pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
pub const ISKRA_HOME_DIR: &str = ".iskra-home";
pub const ISKRA_TMP_DIR: &str = ".iskra-tmp";
const SYSTEM_PATH_DIRS: [&str; 2] = ["/usr/bin", "/bin"];

/// Seatbelt profile shipped as a constant (not a user-editable file).
///
/// Later rules win. Defaults stay open (`allow default`) so the toolchain, linker
/// and dyld work; everything that can leak or persist outside the workspace is
/// closed explicitly:
/// - network: denied (including localhost).
/// - mach-lookup: denied as a whole. This closes keychain (securityd, and with it
///   `git credential-osxkeychain`), pasteboard, cfprefsd (`defaults write`) and
///   other daemons that would act outside the file rules. cargo, rustc, the linker
///   and git were verified to work without any mach service (2026-11-20).
/// - Apple Events: denied.
/// - writes: only inside WORKSPACE and a few character devices.
/// - reads: denied under the owner's home, the project root, other users, mounted
///   volumes and the shared temp areas, then re-allowed for WORKSPACE and the
///   pinned toolchain (TOOLCHAIN = the default toolchain's sysroot). Seatbelt
///   matches resolved paths, so a symlink such as a worktree `.env` pointing into
///   another project folder is still denied.
/// - metadata of the *ancestors* of WORKSPACE and TOOLCHAIN only (needed by
///   realpath/getcwd in cargo and rustc); no listing, no siblings.
const SEATBELT_PROFILE: &str = r#"(version 1)
(allow default)
(deny network*)
(deny mach-lookup)
(deny appleevent-send)
(deny file-write*)
(allow file-write*
    (subpath (param "WORKSPACE"))
    (literal "/dev/null")
    (literal "/dev/zero")
    (literal "/dev/tty")
    (literal "/dev/dtracehelper")
    (regex #"^/dev/fd/[0-9]+$"))
(deny file-read*
    (subpath "/Users")
    (subpath "/Volumes")
    (subpath "/private/tmp")
    (subpath "/private/var/folders")
    (subpath (param "HOME"))
    (subpath (param "PROJECT_ROOT")))
(allow file-read*
    (subpath (param "WORKSPACE"))
    (subpath (param "TOOLCHAIN")))
(allow file-read-metadata
    (path-ancestors (param "WORKSPACE"))
    (path-ancestors (param "TOOLCHAIN")))
"#;

/// Hook and config isolation forced on every git invocation (criterion 6).
pub const GIT_FORCED_CONFIG: [&str; 8] = [
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.pager=cat",
    "-c",
    "protocol.allow=never",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Program {
    Ls,
    Cat,
    Git,
    Cargo,
}

impl Program {
    pub const ALL: [Program; 4] = [Program::Ls, Program::Cat, Program::Git, Program::Cargo];

    pub fn name(self) -> &'static str {
        match self {
            Program::Ls => "ls",
            Program::Cat => "cat",
            Program::Git => "git",
            Program::Cargo => "cargo",
        }
    }

    pub fn from_name(name: &str) -> Option<Program> {
        Program::ALL
            .into_iter()
            .find(|program| program.name() == name)
    }

    /// Fixed absolute locations for system programs. cargo is resolved from the
    /// default toolchain's sysroot instead.
    fn system_candidates(self) -> &'static [&'static str] {
        match self {
            Program::Ls => &["/bin/ls", "/usr/bin/ls"],
            Program::Cat => &["/bin/cat", "/usr/bin/cat"],
            Program::Git => &["/usr/bin/git"],
            Program::Cargo => &[],
        }
    }
}

#[derive(Debug, Clone)]
struct Seatbelt {
    exec: PathBuf,
    home: String,
    project_root: String,
    workspace: String,
    toolchain: String,
}

impl Seatbelt {
    fn params(&self) -> [(&'static str, &str); 4] {
        [
            ("WORKSPACE", &self.workspace),
            ("PROJECT_ROOT", &self.project_root),
            ("HOME", &self.home),
            ("TOOLCHAIN", &self.toolchain),
        ]
    }

    fn apply(&self, command: &mut Command) {
        command.arg("-p").arg(SEATBELT_PROFILE);
        for (key, value) in self.params() {
            command.arg("-D").arg(format!("{key}={value}"));
        }
    }

    /// Run `/usr/bin/true` under the profile once; any failure means the sandbox
    /// cannot be applied on this machine.
    fn probe(&self) -> bool {
        let mut command = std::process::Command::new(&self.exec);
        command.arg("-p").arg(SEATBELT_PROFILE);
        for (key, value) in self.params() {
            command.arg("-D").arg(format!("{key}={value}"));
        }
        command
            .arg("/usr/bin/true")
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

/// Everything decided once at startup (`Tools::new`) and pinned for the process.
pub struct Runtime {
    project_root: PathBuf,
    iskra_home: PathBuf,
    iskra_tmp: PathBuf,
    cargo_home: PathBuf,
    rustup_home: PathBuf,
    toolchain_bin: Option<PathBuf>,
    programs: Vec<(Program, Result<PathBuf, String>)>,
    seatbelt: Option<Seatbelt>,
}

impl Runtime {
    /// Production detection: real Seatbelt binary, real home directory.
    pub fn detect(workspace: &Path) -> io::Result<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Self::detect_with(workspace, Path::new(SANDBOX_EXEC), home.as_deref())
    }

    /// `workspace` must already be canonical. `deny_home` is the directory whose
    /// reads the sandbox denies (the owner's home in production).
    pub fn detect_with(
        workspace: &Path,
        sandbox_exec: &Path,
        deny_home: Option<&Path>,
    ) -> io::Result<Self> {
        let project_root = workspace
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| io::Error::other("workspace has no parent directory"))?;
        let iskra_home = workspace.join(ISKRA_HOME_DIR);
        let iskra_tmp = workspace.join(ISKRA_TMP_DIR);
        let cargo_home = iskra_home.join("cargo");
        fs::create_dir_all(&cargo_home)?;
        fs::create_dir_all(&iskra_tmp)?;

        let rustup_home = real_rustup_home().unwrap_or_else(|| iskra_home.join("rustup"));
        let sysroot = toolchain_sysroot().clone();
        let toolchain_bin = sysroot
            .as_ref()
            .map(|sysroot| sysroot.join("bin"))
            .filter(|bin| bin.to_str().is_some_and(|s| !s.contains(':')));

        let seatbelt = seatbelt_for(workspace, &project_root, sandbox_exec, deny_home, &sysroot);

        let mut programs = Vec::new();
        for program in Program::ALL {
            let pinned = match program {
                Program::Cargo => match (&seatbelt, &toolchain_bin) {
                    (None, _) => Err(
                        "cargo is disabled: the OS sandbox (sandbox-exec) is not available on this machine, and cargo runs build scripts, so it never runs unsandboxed"
                            .to_string(),
                    ),
                    (Some(_), None) => Err(
                        "cargo is disabled: no Rust toolchain was found at startup".to_string(),
                    ),
                    (Some(_), Some(bin)) => existing_file(&bin.join("cargo"))
                        .ok_or_else(|| "cargo is disabled: the toolchain has no cargo binary".to_string()),
                },
                _ => program
                    .system_candidates()
                    .iter()
                    .find_map(|candidate| existing_file(Path::new(candidate)))
                    .ok_or_else(|| format!("{} is disabled: program not found at startup", program.name())),
            };
            if let Err(reason) = &pinned {
                crate::logging::append(&format!(
                    "run_command_program_disabled program={} reason={reason}",
                    program.name()
                ));
            }
            programs.push((program, pinned));
        }
        crate::logging::append(&format!(
            "run_command_sandbox available={}",
            seatbelt.is_some()
        ));

        Ok(Self {
            project_root,
            iskra_home,
            iskra_tmp,
            cargo_home,
            rustup_home,
            toolchain_bin,
            programs,
            seatbelt,
        })
    }

    pub fn sandboxed(&self) -> bool {
        self.seatbelt.is_some()
    }

    pub fn available(&self) -> Vec<Program> {
        self.programs
            .iter()
            .filter(|(_, pinned)| pinned.is_ok())
            .map(|(program, _)| *program)
            .collect()
    }

    pub fn pinned(&self, program: Program) -> Result<&Path, String> {
        self.programs
            .iter()
            .find(|(candidate, _)| *candidate == program)
            .map(|(_, pinned)| pinned.as_deref().map_err(Clone::clone))
            .unwrap_or_else(|| Err(format!("{} is not configured", program.name())))
    }

    /// Build the command for an allowed program whose arguments already passed the
    /// policy. git gets the forced isolation options in front of the model's args.
    pub fn command(
        &self,
        program: Program,
        args: &[String],
        cwd: &Path,
    ) -> Result<Command, String> {
        let exe = self.pinned(program)?.to_path_buf();
        let mut full_args: Vec<String> = Vec::new();
        if program == Program::Git {
            full_args.extend(GIT_FORCED_CONFIG.iter().map(|s| s.to_string()));
        }
        full_args.extend(args.iter().cloned());
        if program == Program::Cargo && self.seatbelt.is_none() {
            return Err("cargo never runs without the OS sandbox".to_string());
        }
        Ok(self.command_for_exe(&exe, &full_args, cwd, true))
    }

    /// Low-level construction shared by `command` and the process tests.
    pub fn command_for_exe(
        &self,
        exe: &Path,
        args: &[String],
        cwd: &Path,
        sandboxed: bool,
    ) -> Command {
        let mut command = match (&self.seatbelt, sandboxed) {
            (Some(seatbelt), true) => {
                let mut command = Command::new(&seatbelt.exec);
                seatbelt.apply(&mut command);
                command.arg(exe);
                command
            }
            _ => Command::new(exe),
        };
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(self.child_env())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        command
    }

    /// The complete child environment (criterion 10). Nothing is inherited.
    pub fn child_env(&self) -> Vec<(&'static str, OsString)> {
        let mut path_dirs: Vec<String> = Vec::new();
        if let Some(bin) = self.toolchain_bin.as_ref().and_then(|bin| bin.to_str()) {
            path_dirs.push(bin.to_string());
        }
        path_dirs.extend(SYSTEM_PATH_DIRS.iter().map(|s| s.to_string()));

        vec![
            ("PATH", OsString::from(path_dirs.join(":"))),
            ("HOME", self.iskra_home.clone().into_os_string()),
            ("LANG", OsString::from("en_US.UTF-8")),
            ("TMPDIR", self.iskra_tmp.clone().into_os_string()),
            ("CARGO_HOME", self.cargo_home.clone().into_os_string()),
            ("RUSTUP_HOME", self.rustup_home.clone().into_os_string()),
            ("CARGO_NET_OFFLINE", OsString::from("true")),
            ("GIT_CONFIG_NOSYSTEM", OsString::from("1")),
            ("GIT_CONFIG_GLOBAL", OsString::from("/dev/null")),
            ("GIT_TERMINAL_PROMPT", OsString::from("0")),
            ("GIT_ASKPASS", OsString::new()),
            (
                "GIT_CEILING_DIRECTORIES",
                self.project_root.clone().into_os_string(),
            ),
            ("GIT_PAGER", OsString::from("cat")),
            ("GIT_EDITOR", OsString::from("true")),
            ("GIT_AUTHOR_NAME", OsString::from("Iskra")),
            ("GIT_AUTHOR_EMAIL", OsString::from("iskra@localhost")),
            ("GIT_COMMITTER_NAME", OsString::from("Iskra")),
            ("GIT_COMMITTER_EMAIL", OsString::from("iskra@localhost")),
        ]
    }
}

/// Kills the child's whole process group (criterion 12). The child is spawned
/// with `process_group(0)`, so its pid is the group id; grandchildren inherit it.
pub struct ProcessGroup(libc::pid_t);

impl ProcessGroup {
    pub fn new(pid: u32) -> Option<Self> {
        let pid = libc::pid_t::try_from(pid).ok()?;
        (pid > 1).then_some(Self(pid))
    }

    pub fn kill(&self) {
        // SAFETY: killpg only sends a signal; the group id is a spawned child's pid (> 1).
        unsafe {
            libc::killpg(self.0, libc::SIGKILL);
        }
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}

fn existing_file(path: &Path) -> Option<PathBuf> {
    let canonical = fs::canonicalize(path).ok()?;
    canonical.is_file().then_some(path.to_path_buf())
}

fn seatbelt_for(
    workspace: &Path,
    project_root: &Path,
    sandbox_exec: &Path,
    deny_home: Option<&Path>,
    sysroot: &Option<PathBuf>,
) -> Option<Seatbelt> {
    if !cfg!(target_os = "macos") || !sandbox_exec.is_file() {
        return None;
    }
    let home = fs::canonicalize(deny_home?).ok()?;
    // Without a toolchain the TOOLCHAIN slot points at the workspace (already allowed).
    let toolchain = sysroot.clone().unwrap_or_else(|| workspace.to_path_buf());
    let seatbelt = Seatbelt {
        exec: sandbox_exec.to_path_buf(),
        home: home.to_str()?.to_string(),
        project_root: project_root.to_str()?.to_string(),
        workspace: workspace.to_str()?.to_string(),
        toolchain: toolchain.to_str()?.to_string(),
    };
    seatbelt.probe().then_some(seatbelt)
}

fn real_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn real_rustup_home() -> Option<PathBuf> {
    std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| real_home().map(|home| home.join(".rustup")))
        .and_then(|p| fs::canonicalize(p).ok())
}

/// The default toolchain's sysroot, found once per process by asking the rustc
/// proxy with fixed arguments, a cleared environment and cwd `/` (so no
/// `rust-toolchain.toml` override can apply). cargo is then run directly from
/// `<sysroot>/bin`, bypassing the rustup proxy (`+toolchain` and toolchain files
/// have no effect).
fn toolchain_sysroot() -> &'static Option<PathBuf> {
    static SYSROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    SYSROOT.get_or_init(|| {
        let home = real_home()?;
        let cargo_home = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".cargo"));
        let rustc = existing_file(&cargo_home.join("bin").join("rustc")).or_else(|| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .filter(|dir| dir.is_absolute())
                    .find_map(|dir| existing_file(&dir.join("rustc")))
            })
        })?;
        let mut command = std::process::Command::new(rustc);
        command
            .args(["--print", "sysroot"])
            .env_clear()
            .env("HOME", &home)
            .env("CARGO_HOME", &cargo_home)
            .env("PATH", SYSTEM_PATH_DIRS.join(":"))
            .current_dir("/")
            .stdin(Stdio::null());
        if let Some(rustup_home) = real_rustup_home() {
            command.env("RUSTUP_HOME", rustup_home);
        }
        let output = command.output().ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        let sysroot = fs::canonicalize(text.trim()).ok()?;
        sysroot.is_dir().then_some(sysroot)
    })
}
