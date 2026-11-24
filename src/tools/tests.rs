use super::sandbox::{GIT_FORCED_CONFIG, SANDBOX_EXEC};
use super::*;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::fs::{PermissionsExt, symlink};

fn temp_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("iskra-tools-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(&dir).unwrap()
}

fn tools_for(root: PathBuf) -> Tools {
    let runtime = Runtime::detect(&root).unwrap();
    let memory = root.join("memory");
    fs::create_dir_all(&memory).unwrap();
    Tools::from_parts(root, runtime, memory)
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// A plain (unsandboxed) process built through the same construction path as
/// run_command: cleared env, pinned absolute program, own process group.
fn plain_command(tools: &Tools, exe: &str, args: &[&str]) -> Command {
    tools
        .runtime
        .command_for_exe(Path::new(exe), &strings(args), &tools.root, false)
}

async fn run(tools: &Tools, program: &str, args: &[&str]) -> Result<String, String> {
    run_in(tools, program, args, None).await
}

async fn run_in(
    tools: &Tools,
    program: &str,
    args: &[&str],
    dir: Option<&str>,
) -> Result<String, String> {
    let mut value = serde_json::json!({"program": program, "args": args});
    if let Some(dir) = dir {
        value["dir"] = serde_json::json!(dir);
    }
    tools.execute("run_command", &value).await
}

fn command_args(command: &Command) -> Vec<String> {
    command
        .as_std()
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect()
}

fn command_envs(command: &Command) -> HashMap<String, Option<String>> {
    command
        .as_std()
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().to_string(),
                value.map(|v| v.to_string_lossy().to_string()),
            )
        })
        .collect()
}

fn pid_alive(pid: libc::pid_t) -> bool {
    // SAFETY: signal 0 only checks for existence.
    unsafe { libc::kill(pid, 0) == 0 }
}

async fn wait_until_gone(pid: libc::pid_t) -> bool {
    for _ in 0..60 {
        if !pid_alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

// ---------------------------------------------------------------------------
// Existing path tests

#[test]
fn allows_paths_inside_workspace() {
    let root = temp_root("inside");
    let tools = tools_for(root.clone());

    assert_eq!(tools.resolve("a.txt").unwrap(), root.join("a.txt"));
    assert_eq!(tools.resolve("sub/b.txt").unwrap(), root.join("sub/b.txt"));
    assert_eq!(tools.resolve("./a.txt").unwrap(), root.join("a.txt"));

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn rejects_parent_escape() {
    let root = temp_root("parent");
    let tools = tools_for(root.clone());

    assert!(tools.resolve("../secret.txt").is_err());
    assert!(tools.resolve("sub/../../secret.txt").is_err());

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn rejects_absolute_path() {
    let root = temp_root("absolute");
    let tools = tools_for(root.clone());

    assert!(tools.resolve("/etc/passwd").is_err());

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn rejects_symlink_escape() {
    let root = temp_root("symlink");
    let outside = std::env::temp_dir().join(format!("iskra-outside-{}", std::process::id()));
    let _ = fs::remove_dir_all(&outside);
    fs::create_dir_all(&outside).unwrap();

    symlink(&outside, root.join("link")).unwrap();
    symlink(outside.join("missing"), root.join("broken")).unwrap();

    let tools = tools_for(root.clone());
    assert!(tools.resolve("link/secret.txt").is_err());
    assert!(tools.resolve("broken").is_err());

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside);
}

#[tokio::test]
async fn rejects_programs_outside_allowlist() {
    let root = temp_root("reject");
    let tools = tools_for(root.clone());

    for program in ["rm", "curl", "sh", "/bin/ls", "ls "] {
        let result = run(&tools, program, &[]).await;
        assert!(result.is_err(), "{program} should be rejected");
        let err = result.unwrap_err();
        // The real allowlist check (W-1 is gone, so nothing else refuses first).
        assert!(
            err.starts_with("Program is not allowed"),
            "{program} rejection should name the allowlist: {err}"
        );
    }

    fs::remove_dir_all(&root).unwrap();
}

#[tokio::test]
async fn does_not_interpret_shell_metacharacters() {
    let root = temp_root("metachar");
    fs::write(root.join("safe.txt"), "safe").unwrap();
    let tools = tools_for(root.clone());

    for (name, arg) in [
        ("pwned-semicolon", "safe.txt; touch pwned-semicolon"),
        ("pwned-and", "safe.txt && touch pwned-and"),
        ("pwned-pipe", "safe.txt | touch pwned-pipe"),
    ] {
        let command = plain_command(&tools, "/bin/cat", &[arg]);
        let outcome = run_process(command, "cat", Duration::from_secs(5))
            .await
            .unwrap();
        assert!(!outcome.text.starts_with("exit=0"));
        assert!(!root.join(name).exists(), "{name} must not be created");
    }

    fs::remove_dir_all(&root).unwrap();
}

#[tokio::test]
async fn caps_command_output() {
    let root = temp_root("output-cap");
    let content = "a".repeat(OUTPUT_LIMIT_CHARS + 2_000);
    fs::write(root.join("big.txt"), content).unwrap();
    let tools = tools_for(root.clone());

    let command = plain_command(&tools, "/bin/cat", &["big.txt"]);
    let outcome = run_process(command, "cat", Duration::from_secs(5))
        .await
        .unwrap();

    assert!(
        outcome
            .text
            .contains("output truncated at 10,000 characters")
    );
    let body = outcome.text.split("\n...(").next().unwrap_or(&outcome.text);
    let a_count = body.chars().filter(|c| *c == 'a').count();
    assert_eq!(a_count, OUTPUT_LIMIT_CHARS);

    fs::remove_dir_all(&root).unwrap();
}

#[tokio::test]
async fn kills_command_on_timeout() {
    let root = temp_root("timeout");
    let tools = tools_for(root.clone());
    let started = std::time::Instant::now();

    let command = plain_command(&tools, "/bin/sleep", &["5"]);
    let outcome = run_process(command, "sleep", Duration::from_millis(100))
        .await
        .unwrap();

    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(outcome.text.contains("Timed out"));
    assert_eq!(outcome.code, "timeout");

    fs::remove_dir_all(&root).unwrap();
}

#[tokio::test]
async fn runs_commands_inside_workspace() {
    let root = temp_root("cwd");
    fs::write(root.join("inside.txt"), "inside-content").unwrap();
    let tools = tools_for(root.clone());

    let outcome = run(&tools, "cat", &["inside.txt"]).await.unwrap();

    assert!(outcome.starts_with("exit=0"), "{outcome}");
    assert!(outcome.contains("inside-content"));

    fs::remove_dir_all(&root).unwrap();
}

// ---------------------------------------------------------------------------
// v1.1-A criterion 1: outside paths are refused before spawn

#[tokio::test]
async fn refuses_outside_paths_before_spawn() {
    let root = temp_root("outside-paths");
    let tools = tools_for(root.clone());

    for (program, args) in [
        ("cat", vec!["../.env"]),
        ("cat", vec!["/etc/hosts"]),
        ("ls", vec!["/"]),
        ("ls", vec![".."]),
        ("git", vec!["show", "../.env"]),
        ("cargo", vec!["build", "--manifest-path", "../Cargo.toml"]),
    ] {
        let result = run(&tools, program, &args).await;
        // Refusals are Err; a spawned process always returns Ok("exit=…").
        assert!(
            result.as_ref().is_err_and(|e| e.contains("refused")),
            "{program} {args:?} must be refused before spawn: {result:?}"
        );
    }

    fs::remove_dir_all(&root).unwrap();
}

// Criterion 2: absolute, `..` and `~` anywhere in an argument

#[test]
fn refuses_absolute_parent_and_home_arguments() {
    for arg in [
        "/etc/hosts",
        "../x",
        "a/../../x",
        "..",
        "~",
        "~/.ssh/id_rsa",
        "--output=/tmp/x",
        "--output=../x",
        "--manifest-path=/x/Cargo.toml",
        "--target-dir=~/x",
        "-O/etc/passwd",
        "-o../x",
        "HEAD:../x",
        ":/message-search",
    ] {
        assert!(check_path_argument(arg).is_err(), "{arg} must be refused");
    }
    for arg in [
        "src/lib.rs",
        "HEAD~3..HEAD",
        "main..feature",
        "--format=%H %s",
        "-la",
        "HEAD:src/lib.rs",
    ] {
        assert!(check_path_argument(arg).is_ok(), "{arg} must be allowed");
    }

    let args = |list: &[&str]| strings(list);
    assert!(check_program_args(Program::Git, &args(&["log", "--output=/tmp/x"])).is_err());
    assert!(check_program_args(Program::Git, &args(&["diff", "--output=../x"])).is_err());
}

// Criterion 3: git global options before the subcommand

#[test]
fn git_refuses_global_options_before_subcommand() {
    for global in [
        "-c",
        "-C",
        "--git-dir=x",
        "--work-tree=x",
        "--exec-path=x",
        "--namespace=x",
        "--config-env=a=b",
        "--super-prefix=x",
        "-p",
        "--paginate",
        "--no-pager",
    ] {
        let args = strings(&[global, "status"]);
        let result = check_program_args(Program::Git, &args);
        assert!(
            result
                .as_ref()
                .is_err_and(|e| e.contains("before the subcommand")),
            "{global} must be refused: {result:?}"
        );
    }
    assert!(check_program_args(Program::Git, &[]).is_err());
}

// Criterion 4: git subcommand allowlist

#[test]
fn git_refuses_subcommands_outside_allowlist() {
    for subcommand in [
        "push",
        "fetch",
        "pull",
        "clone",
        "ls-remote",
        "remote",
        "submodule",
        "config",
        "clean",
        "reset",
        "checkout",
        "restore",
        "apply",
        "am",
        "archive",
        "bundle",
        "daemon",
        "instaweb",
        "filter-branch",
        "credential",
        "stash",
        "x",
    ] {
        let args = strings(&[subcommand]);
        let result = check_program_args(Program::Git, &args);
        assert!(
            result.as_ref().is_err_and(|e| e.contains("not allowed")),
            "git {subcommand} must be refused: {result:?}"
        );
    }
    for subcommand in GIT_SUBCOMMANDS {
        assert!(check_program_args(Program::Git, &strings(&[subcommand])).is_ok());
    }
}

// Criterion 5: options that execute code or write elsewhere, including git's
// unique-prefix abbreviations

#[test]
fn git_refuses_code_running_and_redirecting_options() {
    for (subcommand, option) in [
        ("log", "--output=out.txt"),
        ("log", "--outp=out.txt"),
        ("diff", "--ext-diff"),
        ("diff", "--ext-d"),
        ("show", "--textconv"),
        ("show", "--textc"),
        ("log", "--exec=x"),
        ("init", "--template=tpl"),
        ("status", "--upload-pack=x"),
        ("status", "--receive-pack=x"),
        ("log", "--config=a.b=c"),
        ("log", "-c"),
        ("log", "-calias.x=!sh"),
        ("commit", "-S"),
        ("commit", "--gpg-sign"),
        ("log", "--show-signature"),
    ] {
        let args = strings(&[subcommand, option]);
        assert!(
            check_program_args(Program::Git, &args).is_err(),
            "git {subcommand} {option} must be refused"
        );
    }
    for (subcommand, option) in [
        ("commit", "--no-verify"),
        ("log", "--oneline"),
        ("diff", "--no-ext-diff"),
        ("diff", "--cached"),
        ("log", "--output-indicator-new=+"),
        ("commit", "-m"),
    ] {
        let args = strings(&[subcommand, option]);
        assert!(
            check_program_args(Program::Git, &args).is_ok(),
            "git {subcommand} {option} must stay allowed"
        );
    }
}

// Criterion 6: hook/config isolation is forced by Iskra on the Command

#[test]
fn git_command_forces_hook_and_config_isolation() {
    let root = temp_root("git-isolation");
    let tools = tools_for(root.clone());
    let git = tools.runtime.pinned(Program::Git).unwrap().to_path_buf();

    let command = tools
        .runtime
        .command(Program::Git, &strings(&["status"]), &root)
        .unwrap();
    let args = command_args(&command);
    let start = if tools.runtime.sandboxed() {
        assert_eq!(command.as_std().get_program(), OsStr::new(SANDBOX_EXEC));
        args.iter()
            .position(|arg| Path::new(arg) == git)
            .expect("pinned git path follows the sandbox profile")
            + 1
    } else {
        assert_eq!(command.as_std().get_program(), git.as_os_str());
        0
    };
    assert_eq!(
        &args[start..start + 8],
        &GIT_FORCED_CONFIG.map(String::from)
    );
    assert_eq!(args[start + 8], "status");

    let envs = command_envs(&command);
    for (key, value) in [
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_ASKPASS", ""),
    ] {
        assert_eq!(envs.get(key), Some(&Some(value.to_string())), "{key}");
    }
    assert_eq!(
        envs.get("GIT_CEILING_DIRECTORIES"),
        Some(&Some(root.parent().unwrap().to_string_lossy().to_string()))
    );

    fs::remove_dir_all(&root).unwrap();
}

#[tokio::test]
async fn git_does_not_discover_parent_repository() {
    let base = temp_root("git-parent");
    let root = base.join("workspace");
    fs::create_dir_all(&root).unwrap();
    // A repository in the project root (the workspace's parent), like Iskra's own.
    let status = std::process::Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .current_dir(&base)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap();
    assert!(status.success());
    let tools = tools_for(root.clone());

    let outcome = run(&tools, "git", &["status"]).await.unwrap();
    assert!(!outcome.starts_with("exit=0"), "{outcome}");
    assert!(outcome.contains("not a git repository"), "{outcome}");

    // A repository inside workspace still works through `dir`.
    fs::create_dir_all(root.join("repo")).unwrap();
    let init = run_in(&tools, "git", &["init", "-q"], Some("repo"))
        .await
        .unwrap();
    assert!(init.starts_with("exit=0"), "{init}");
    fs::write(root.join("repo/a.txt"), "a").unwrap();
    let status = run_in(&tools, "git", &["status", "--short"], Some("repo"))
        .await
        .unwrap();
    assert!(
        status.starts_with("exit=0") && status.contains("a.txt"),
        "{status}"
    );

    fs::remove_dir_all(&base).unwrap();
}

// Criterion 7: cargo subcommand allowlist, toolchain overrides, unstable flags

#[test]
fn cargo_refuses_subcommands_toolchains_and_unstable_flags() {
    for args in [
        vec!["install", "ripgrep"],
        vec!["publish"],
        vec!["login"],
        vec!["logout"],
        vec!["owner"],
        vec!["yank"],
        vec!["fetch"],
        vec!["update"],
        vec!["vendor"],
        vec!["+nightly", "build"],
        vec!["build", "+nightly"],
        vec!["-Zunstable-options", "build"],
        vec!["build", "-Zbuild-std"],
        vec!["build", "-Z", "x"],
        vec!["--config", "build.rustc-wrapper='x'", "build"],
        vec!["build", "--config", "build.rustc-wrapper='x'"],
        vec!["build", "--config=x"],
        vec!["build", "--manifest-path", "/elsewhere/Cargo.toml"],
        vec![],
    ] {
        let list = strings(&args);
        assert!(
            check_program_args(Program::Cargo, &list).is_err(),
            "cargo {args:?} must be refused"
        );
    }
    for subcommand in CARGO_SUBCOMMANDS {
        assert!(check_program_args(Program::Cargo, &strings(&[subcommand])).is_ok());
    }
    assert!(check_program_args(Program::Cargo, &strings(&["test", "--", "--nocapture"])).is_ok());
}

// Criterion 8: cargo runs offline with homes pinned by Iskra

#[test]
fn cargo_command_runs_offline_with_pinned_homes() {
    let root = temp_root("cargo-env");
    let tools = tools_for(root.clone());

    let envs: HashMap<&str, String> = tools
        .runtime
        .child_env()
        .into_iter()
        .map(|(key, value)| (key, value.to_string_lossy().to_string()))
        .collect();
    assert_eq!(envs["CARGO_NET_OFFLINE"], "true");
    assert_eq!(
        envs["CARGO_HOME"],
        root.join(".iskra-home/cargo").to_string_lossy()
    );
    assert!(envs.contains_key("RUSTUP_HOME"));

    if tools.runtime.pinned(Program::Cargo).is_ok() {
        let command = tools
            .runtime
            .command(Program::Cargo, &strings(&["build"]), &root)
            .unwrap();
        assert_eq!(command.as_std().get_program(), OsStr::new(SANDBOX_EXEC));
        assert_eq!(
            command_envs(&command).get("CARGO_NET_OFFLINE"),
            Some(&Some("true".to_string()))
        );
    }

    fs::remove_dir_all(&root).unwrap();
}

// Criterion 9: write_file refuses config-driven execution paths

#[tokio::test]
async fn write_file_refuses_git_cargo_and_toolchain_files() {
    let root = temp_root("write-refusal");
    let tools = tools_for(root.clone());
    fs::create_dir_all(root.join("repo/.git")).unwrap();
    symlink(root.join("repo/.git"), root.join("gitdir-link")).unwrap();

    for path in [
        ".git",
        ".git/config",
        "repo/.git/hooks/pre-commit",
        "repo/.GIT/config",
        ".Git/hooks/post-checkout",
        ".gitattributes",
        "sub/.gitmodules",
        ".gitconfig",
        ".github/workflows/x.yml",
        ".cargo/config.toml",
        "crate/.CARGO/config",
        "rust-toolchain",
        "crate/rust-toolchain.toml",
        "crate/RUST-TOOLCHAIN.TOML",
        "ruſt-toolchain",
        "gitdir-link/config",
    ] {
        let args = serde_json::json!({"path": path, "content": "[core]\n"});
        let result = tools.execute("write_file", &args).await;
        assert!(
            result.as_ref().is_err_and(|e| e.contains("refused")),
            "{path} must be refused: {result:?}"
        );
    }
    assert!(!root.join(".git").exists());
    assert!(!root.join("repo/.git/config").exists());

    let ok = serde_json::json!({"path": "notes/git-tips.md", "content": "ok"});
    assert!(tools.execute("write_file", &ok).await.is_ok());

    fs::remove_dir_all(&root).unwrap();
}

#[tokio::test]
async fn file_tools_refuse_private_iskra_dirs() {
    let root = temp_root("private-dirs");
    let tools = tools_for(root.clone()); // creates .iskra-home/ and .iskra-tmp/
    fs::write(root.join(".iskra-tmp/probe.txt"), "x").unwrap();

    for path in [
        ".iskra-home/cargo/config.toml",
        ".ISKRA-TMP/probe.txt",
        ".iſkra-home/x",
    ] {
        let write = serde_json::json!({"path": path, "content": "x"});
        assert!(
            tools.execute("write_file", &write).await.is_err(),
            "write {path}"
        );
        let read = serde_json::json!({"path": path});
        assert!(
            tools.execute("read_file", &read).await.is_err(),
            "read {path}"
        );
    }
    assert!(run(&tools, "cat", &[".iskra-tmp/probe.txt"]).await.is_err());
    assert!(
        run_in(&tools, "ls", &[], Some(".iskra-home"))
            .await
            .is_err()
    );

    fs::remove_dir_all(&root).unwrap();
}

// Criterion 10: the child sees no inherited environment variable

#[tokio::test]
async fn child_environment_does_not_inherit_parent_secrets() {
    let root = temp_root("env-clear");
    let tools = tools_for(root.clone());
    // SAFETY: test-only; these names are not read by other tests.
    unsafe {
        std::env::set_var("ISKRA_TEST_SECRET", "iskra-test-secret-value");
        std::env::set_var("OPENAI_API_KEY", "sk-iskra-test-not-real-0000000000");
        std::env::set_var("SSH_AUTH_SOCK", "/tmp/iskra-test-agent.sock");
    }

    let command = tools
        .runtime
        .command_for_exe(Path::new("/usr/bin/env"), &[], &root, true);
    let outcome = run_process(command, "env", Duration::from_secs(5))
        .await
        .unwrap();

    assert!(outcome.text.starts_with("exit=0"), "{}", outcome.text);
    for name in [
        "ISKRA_TEST_SECRET",
        "OPENAI_API_KEY",
        "SSH_AUTH_SOCK",
        "DEEPSEEK_API_KEY",
        "TELEGRAM_BOT_TOKEN",
        "LLM_API_KEY",
        "USER",
    ] {
        assert!(
            !outcome
                .text
                .lines()
                .any(|line| line.starts_with(&format!("{name}="))),
            "{name} leaked into the child: {}",
            outcome.text
        );
    }
    let expected: Vec<String> = tools
        .runtime
        .child_env()
        .iter()
        .map(|(key, _)| key.to_string())
        .collect();
    for line in outcome.text.lines().skip(1) {
        let key = line.split('=').next().unwrap_or("");
        assert!(
            expected.iter().any(|name| name == key) || key == "__CF_USER_TEXT_ENCODING",
            "unexpected child variable: {line}"
        );
    }
    let path_line = outcome
        .text
        .lines()
        .find(|line| line.starts_with("PATH="))
        .unwrap();
    assert!(
        path_line[5..].split(':').all(|dir| dir.starts_with('/')),
        "{path_line}"
    );

    fs::remove_dir_all(&root).unwrap();
}

// Criterion 11: programs are pinned at startup, never looked up at call time

#[tokio::test]
async fn programs_are_pinned_and_never_resolved_from_workspace() {
    let root = temp_root("pinned");
    for name in ["git", "ls", "cat"] {
        let script = root.join(name);
        fs::write(&script, format!("#!/bin/sh\ntouch pwned-{name}\n")).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let tools = tools_for(root.clone());

    for program in [Program::Git, Program::Ls, Program::Cat] {
        let pinned = tools.runtime.pinned(program).unwrap();
        assert!(pinned.is_absolute(), "{pinned:?}");
        assert!(!pinned.starts_with(&root), "{pinned:?}");
    }
    let _ = run(&tools, "git", &["status"]).await;
    let _ = run(&tools, "ls", &[]).await;
    let _ = run(&tools, "cat", &["git"]).await;
    for name in ["git", "ls", "cat"] {
        assert!(
            !root.join(format!("pwned-{name}")).exists(),
            "workspace/{name} was executed"
        );
    }

    fs::remove_dir_all(&root).unwrap();
}

// Criterion 12: the whole process group dies on timeout and on abort (/stop)

#[tokio::test]
async fn timeout_kills_whole_process_group() {
    let root = temp_root("group-timeout");
    let tools = tools_for(root.clone());

    let command = plain_command(&tools, "/bin/sh", &["-c", "sleep 60 & echo $!; wait"]);
    let outcome = run_process(command, "sh", Duration::from_millis(500))
        .await
        .unwrap();
    assert_eq!(outcome.code, "timeout");
    let pid: libc::pid_t = outcome
        .text
        .lines()
        .find_map(|line| line.trim().parse().ok())
        .expect("grandchild pid in output");
    assert!(
        wait_until_gone(pid).await,
        "grandchild {pid} survived the timeout"
    );

    fs::remove_dir_all(&root).unwrap();
}

#[tokio::test]
async fn abort_kills_whole_process_group() {
    let root = temp_root("group-abort");
    let tools = tools_for(root.clone());

    let command = plain_command(
        &tools,
        "/bin/sh",
        &["-c", "sleep 60 & echo $! > gc.pid; wait"],
    );
    let handle =
        tokio::spawn(async move { run_process(command, "sh", Duration::from_secs(60)).await });
    let pid_file = root.join("gc.pid");
    let mut pid: Option<libc::pid_t> = None;
    for _ in 0..60 {
        if let Ok(text) = fs::read_to_string(&pid_file)
            && let Ok(value) = text.trim().parse()
        {
            pid = Some(value);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let pid = pid.expect("grandchild pid file");
    assert!(pid_alive(pid));

    handle.abort(); // what /stop does to the running turn
    let _ = handle.await;
    assert!(
        wait_until_gone(pid).await,
        "grandchild {pid} survived the abort"
    );

    fs::remove_dir_all(&root).unwrap();
}

// Criterion 13: read_file never follows a final symlink and refuses hard links

#[tokio::test]
async fn read_file_refuses_hard_links_and_final_symlinks() {
    let root = temp_root("read-links");
    let outside = std::env::temp_dir().join(format!("iskra-read-outside-{}", std::process::id()));
    let _ = fs::remove_dir_all(&outside);
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), "outside-secret").unwrap();
    fs::hard_link(outside.join("secret.txt"), root.join("hard.txt")).unwrap();
    fs::write(root.join("plain.txt"), "plain").unwrap();
    symlink(root.join("plain.txt"), root.join("final-link")).unwrap();
    let tools = tools_for(root.clone());

    let result = tools
        .execute("read_file", &serde_json::json!({"path": "hard.txt"}))
        .await;
    assert!(
        result.as_ref().is_err_and(|e| e.contains("Hard-linked")),
        "{result:?}"
    );
    assert!(run(&tools, "cat", &["hard.txt"]).await.is_err());
    let write = serde_json::json!({"path": "hard.txt", "content": "overwrite"});
    assert!(tools.execute("write_file", &write).await.is_err());
    assert_eq!(
        fs::read_to_string(outside.join("secret.txt")).unwrap(),
        "outside-secret"
    );

    assert!(open_regular_nofollow(&root.join("final-link")).is_err());
    let plain = tools
        .execute("read_file", &serde_json::json!({"path": "plain.txt"}))
        .await;
    assert_eq!(plain.unwrap(), "plain");

    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(&outside).unwrap();
}

#[tokio::test]
async fn ls_and_cat_refuse_escaping_operands_and_flags() {
    let root = temp_root("ls-cat");
    let outside = std::env::temp_dir().join(format!("iskra-lscat-outside-{}", std::process::id()));
    let _ = fs::remove_dir_all(&outside);
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, root.join("out-link")).unwrap();
    fs::write(root.join("a.txt"), "a").unwrap();
    let tools = tools_for(root.clone());

    assert!(run(&tools, "ls", &["out-link"]).await.is_err());
    assert!(run(&tools, "cat", &["out-link/x"]).await.is_err());
    assert!(run(&tools, "ls", &["-L"]).await.is_err());
    assert!(run(&tools, "ls", &["-lH", "."]).await.is_err());
    assert!(run(&tools, "ls", &["--dereference"]).await.is_err());
    assert!(run(&tools, "cat", &["a.txt", "out-link"]).await.is_err());
    assert!(run_in(&tools, "ls", &[], Some("out-link")).await.is_err());
    assert!(run_in(&tools, "ls", &[], Some("../")).await.is_err());

    let listing = run(&tools, "ls", &["-la"]).await.unwrap();
    assert!(
        listing.starts_with("exit=0") && listing.contains("a.txt"),
        "{listing}"
    );

    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(&outside).unwrap();
}

// Criterion 15: no OS sandbox → cargo disabled and not advertised

#[tokio::test]
async fn cargo_disabled_without_os_sandbox() {
    let root = temp_root("no-sandbox");
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let runtime = Runtime::detect_with(
        &root,
        Path::new("/nonexistent/sandbox-exec"),
        home.as_deref(),
    )
    .unwrap();
    assert!(!runtime.sandboxed());
    let reason = runtime.pinned(Program::Cargo).unwrap_err();
    assert!(reason.contains("sandbox"), "{reason}");
    let available = runtime.available();
    assert!(!available.contains(&Program::Cargo));

    let schema = schemas_for(&available, runtime.sandboxed());
    let run_command = schema
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["function"]["name"] == "run_command")
        .expect("run_command advertised for the remaining programs");
    let programs = run_command["function"]["parameters"]["properties"]["program"]["enum"]
        .as_array()
        .unwrap();
    assert!(!programs.iter().any(|p| p == "cargo"), "{programs:?}");
    let description = run_command["function"]["description"].as_str().unwrap();
    assert!(!description.contains("cargo"), "{description}");
    assert!(!description.contains("OS sandbox"), "{description}");

    let tools = Tools::from_parts(root.clone(), runtime, root.join("memory"));
    let result = run(&tools, "cargo", &["build"]).await;
    assert!(
        result.as_ref().is_err_and(|e| e.contains("not available")),
        "{result:?}"
    );

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn tools_instances_own_their_capability_advertisement() {
    let root_a = temp_root("instance-advert-a");
    let root_b = temp_root("instance-advert-b");
    let mut a = tools_for(root_a.clone());
    let mut b = tools_for(root_b.clone());
    a.advert = Advert {
        programs: Program::ALL.to_vec(),
        sandboxed: true,
    };
    b.advert = Advert {
        programs: Vec::new(),
        sandboxed: false,
    };

    let names = |tools: &Tools| -> Vec<String> {
        tools
            .schemas()
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(names(&a).iter().any(|name| name == "run_command"));
    assert!(!names(&b).iter().any(|name| name == "run_command"));
    assert!(names(&a).iter().any(|name| name == "run_command"));

    fs::remove_dir_all(&root_a).unwrap();
    fs::remove_dir_all(&root_b).unwrap();
}

#[tokio::test]
async fn tools_instances_keep_memory_and_stickies_in_their_own_roots() {
    let root_a = temp_root("instance-memory-a");
    let root_b = temp_root("instance-memory-b");
    let memory_a = temp_root("instance-memory-data-a");
    let memory_b = temp_root("instance-memory-data-b");
    let a = Tools::from_parts(
        root_a.clone(),
        Runtime::detect(&root_a).unwrap(),
        memory_a.clone(),
    );
    let b = Tools::from_parts(
        root_b.clone(),
        Runtime::detect(&root_b).unwrap(),
        memory_b.clone(),
    );

    a.execute("memory_save", &serde_json::json!({"text": "fact-a"}))
        .await
        .unwrap();
    b.execute("memory_save", &serde_json::json!({"text": "fact-b"}))
        .await
        .unwrap();

    let text_a = fs::read_to_string(memory_a.join(crate::memory::MEMORY_FILE)).unwrap();
    let text_b = fs::read_to_string(memory_b.join(crate::memory::MEMORY_FILE)).unwrap();
    assert!(text_a.contains("fact-a") && !text_a.contains("fact-b"));
    assert!(text_b.contains("fact-b") && !text_b.contains("fact-a"));

    a.execute(
        "sticky_note",
        &serde_json::json!({"action": "add", "text": "note-a"}),
    )
    .await
    .unwrap();
    assert!(memory_a.join(crate::stickies::STICKIES_FILE).exists());
    assert!(!memory_b.join(crate::stickies::STICKIES_FILE).exists());

    fs::remove_dir_all(&root_a).unwrap();
    fs::remove_dir_all(&root_b).unwrap();
    fs::remove_dir_all(&memory_a).unwrap();
    fs::remove_dir_all(&memory_b).unwrap();
}

#[test]
fn schema_lists_all_tools_when_every_program_is_available() {
    let schema = schemas_for(&Program::ALL, true);
    let names: Vec<&str> = schema
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "read_file",
            "write_file",
            "memory_save",
            "run_command",
            "sticky_note"
        ]
    );
    assert!(schemas_for(&[], false).as_array().unwrap().len() == 4);
}

// Criterion 16: the run_command log line is redacted

#[test]
fn run_command_log_line_redacts_credentials() {
    let token = "sk-iskraTestToken000000000000000000";
    let line = command_log_line(
        "git",
        &strings(&["commit", "-m", &format!("key {token}")]),
        None,
    );
    assert!(!line.contains(token), "{line}");
    assert!(line.contains("redacted"), "{line}");
}

// ---------------------------------------------------------------------------
// Criterion 14 (macOS): build scripts run inside Seatbelt

/// A temp layout shaped like the owner's worktree setup, with decoys only:
/// `<home>/iskra/.env` and `<home>/iskra-v1.1-a/.env -> ../iskra/.env`, with the
/// workspace at `<home>/iskra-v1.1-a/workspace`. The sandbox denies reads under
/// `<home>` (as it does for the owner's real home).
#[cfg(target_os = "macos")]
struct SandboxLab {
    base: PathBuf,
    home: PathBuf,
    project: PathBuf,
    tools: Tools,
}

#[cfg(target_os = "macos")]
impl SandboxLab {
    fn new(name: &str) -> Self {
        let base = temp_root(name);
        let home = base.join("home");
        let project = home.join("iskra-v1.1-a");
        let root = project.join("workspace");
        fs::create_dir_all(home.join("iskra")).unwrap();
        fs::create_dir_all(&root).unwrap();
        fs::write(home.join("iskra/.env"), "DECOY_SECRET=not-a-real-secret\n").unwrap();
        symlink("../iskra/.env", project.join(".env")).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let runtime = Runtime::detect_with(&root, Path::new(SANDBOX_EXEC), Some(&home)).unwrap();
        assert!(runtime.sandboxed(), "Seatbelt must be available on macOS");
        assert!(
            runtime.pinned(Program::Cargo).is_ok(),
            "cargo must be available"
        );
        let tools = Tools::from_parts(root, runtime, base.join("memory"));
        Self {
            base,
            home,
            project,
            tools,
        }
    }

    fn root(&self) -> &Path {
        &self.tools.root
    }

    /// Writes a crate whose build.rs runs `body`, then builds it via run_command.
    async fn build_probe(&self, name: &str, body: &str) -> String {
        let dir = self.root().join(name);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n\n[lib]\npath = \"src/lib.rs\"\n"
            ),
        )
        .unwrap();
        fs::write(dir.join("src/lib.rs"), "").unwrap();
        fs::write(dir.join("build.rs"), format!("fn main() {{\n{body}\n}}\n")).unwrap();
        run_in(&self.tools, "cargo", &["build", "--offline"], Some(name))
            .await
            .unwrap()
    }
}

#[cfg(target_os = "macos")]
impl Drop for SandboxLab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn sandbox_blocks_build_script_reading_project_env() {
    let lab = SandboxLab::new("sbx-env");
    let leak = lab.root().join("LEAK-env");
    let body = format!(
        "let a = std::fs::read({:?}).expect(\"read project .env via symlink\");\n\
         let b = std::fs::read({:?}).expect(\"read .env target\");\n\
         std::fs::write({:?}, format!(\"{{}} {{}}\", a.len(), b.len())).unwrap();",
        lab.project.join(".env"),
        lab.home.join("iskra/.env"),
        leak
    );
    let outcome = lab.build_probe("probe_env", &body).await;
    assert!(!outcome.starts_with("exit=0"), "{outcome}");
    assert!(outcome.contains("read project .env"), "{outcome}");
    assert!(!leak.exists());
    assert!(!outcome.contains("DECOY_SECRET"));
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn sandbox_blocks_build_script_listing_ssh_dir() {
    let lab = SandboxLab::new("sbx-ssh");
    let real_home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let ssh = real_home.join(".ssh");
    let target = if ssh.exists() { ssh } else { real_home };
    let leak = lab.root().join("LEAK-ssh");
    let body = format!(
        "let n = std::fs::read_dir({target:?}).expect(\"list ~/.ssh\").count();\n\
         std::fs::write({leak:?}, n.to_string()).unwrap();"
    );
    let outcome = lab.build_probe("probe_ssh", &body).await;
    assert!(!outcome.starts_with("exit=0"), "{outcome}");
    assert!(outcome.contains("list ~/.ssh"), "{outcome}");
    assert!(!leak.exists());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn sandbox_blocks_build_script_network() {
    let lab = SandboxLab::new("sbx-net");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let body = format!(
        "let addr: std::net::SocketAddr = \"{address}\".parse().unwrap();\n\
         std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(3)).expect(\"tcp connect\");"
    );
    let outcome = lab.build_probe("probe_net", &body).await;
    assert!(!outcome.starts_with("exit=0"), "{outcome}");
    assert!(outcome.contains("tcp connect"), "{outcome}");
    assert!(
        listener.accept().is_err(),
        "the build script reached the listener"
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn sandbox_blocks_build_script_writing_outside_workspace() {
    let lab = SandboxLab::new("sbx-write");
    let outside = lab.project.join("outside.txt");
    let body = format!("std::fs::write({outside:?}, \"x\").expect(\"write outside workspace\");");
    let outcome = lab.build_probe("probe_write", &body).await;
    assert!(!outcome.starts_with("exit=0"), "{outcome}");
    assert!(outcome.contains("write outside workspace"), "{outcome}");
    assert!(!outside.exists());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn sandbox_blocks_build_script_hard_linking_outside_file() {
    let lab = SandboxLab::new("sbx-link");
    let link = lab.root().join("linked-env");
    let body = format!(
        "std::fs::hard_link({:?}, {link:?}).expect(\"hard link outside file\");",
        lab.home.join("iskra/.env")
    );
    let outcome = lab.build_probe("probe_link", &body).await;
    assert!(!outcome.starts_with("exit=0"), "{outcome}");
    assert!(outcome.contains("hard link outside file"), "{outcome}");
    assert!(!link.exists());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn sandboxed_cargo_and_git_still_work_inside_workspace() {
    let lab = SandboxLab::new("sbx-regression");
    let outcome = lab
        .build_probe("probe_ok", "println!(\"cargo:rerun-if-changed=build.rs\");")
        .await;
    assert!(outcome.starts_with("exit=0"), "{outcome}");

    fs::write(
        lab.root().join("probe_ok/src/lib.rs"),
        "#[test]\nfn adds() { assert_eq!(1 + 1, 2); }\n",
    )
    .unwrap();
    let test = run_in(&lab.tools, "cargo", &["test"], Some("probe_ok"))
        .await
        .unwrap();
    assert!(
        test.starts_with("exit=0") && test.contains("1 passed"),
        "{test}"
    );

    let init = run_in(&lab.tools, "git", &["init", "-q"], Some("probe_ok"))
        .await
        .unwrap();
    assert!(init.starts_with("exit=0"), "{init}");
    let add = run_in(&lab.tools, "git", &["add", "src"], Some("probe_ok"))
        .await
        .unwrap();
    assert!(add.starts_with("exit=0"), "{add}");
    let commit = run_in(
        &lab.tools,
        "git",
        &["commit", "-qm", "init"],
        Some("probe_ok"),
    )
    .await
    .unwrap();
    assert!(commit.starts_with("exit=0"), "{commit}");
    let log = run_in(&lab.tools, "git", &["log", "--oneline"], Some("probe_ok"))
        .await
        .unwrap();
    assert!(log.starts_with("exit=0") && log.contains("init"), "{log}");
}
