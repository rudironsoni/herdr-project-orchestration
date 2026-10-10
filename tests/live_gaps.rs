//! Shipped-binary evidence for the worktree crash window, `o`, and prompts.
//!
//! Herdr is the disposable session `hpo-tui-gap` on the real HOME, so Claude
//! can start. `open` starts the coordinator. Missing, blocked, unknown,
//! refused, and uncertain stay on the stand-in. The confirmed prompt is
//! delegated, and it counts only when that reply reports the bound agent
//! `working` or `blocked`. A finished worktree thread stores its path once.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_herdr-projects");
const DRIVE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/pty_drive.py");
const STANDIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/herdr_standin.py");
const PATH: &str = "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin";
const SESSION: &str = "hpo-tui-gap";

struct Server {
    herdr: PathBuf,
    child: Child,
}

fn herdr_client(herdr: &Path) -> Command {
    let mut cmd = Command::new(herdr);
    for key in [
        "HERDR_SOCKET_PATH",
        "HERDR_SESSION",
        "HERDR_ENV",
        "HERDR_PANE_ID",
        "HERDR_TAB_ID",
        "HERDR_WORKSPACE_ID",
        "HERDR_TERMINAL_ID",
    ] {
        cmd.env_remove(key);
    }
    cmd
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = herdr_client(&self.herdr)
            .args(["session", "stop", SESSION])
            .output();
        let _ = herdr_client(&self.herdr)
            .args(["session", "delete", SESSION])
            .output();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct World {
    server: Server,
    extra: ExtraWorktrees,
    lock: File,
    files: tempfile::TempDir,
    user_home: PathBuf,
    root: PathBuf,
    repo: PathBuf,
    project_dir: PathBuf,
    socket: PathBuf,
    herdr: PathBuf,
    standin: PathBuf,
    mode: PathBuf,
    state: PathBuf,
    log: PathBuf,
    marker: PathBuf,
    focus_marker: PathBuf,
}

fn real_herdr() -> PathBuf {
    let out = Command::new("/bin/sh")
        .env("PATH", PATH)
        .args(["-c", "command -v herdr"])
        .output()
        .unwrap();
    assert!(out.status.success(), "herdr is not on PATH");
    PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
}

fn version(home: &Path) -> String {
    let out = Command::new(BIN)
        .env_clear()
        .env("HOME", home)
        .env("PATH", PATH)
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .last()
        .unwrap()
        .to_string()
}

fn hold_ticker(root: &Path, version: &str) -> File {
    let path = root.join(".ticker.lock");
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    let body = format!(
        "{{\"version\":\"{version}\",\"pid\":1,\"root\":\"{}\",\"started\":\"t\",\"tools\":[]}}",
        root.display()
    );
    file.write_all(body.as_bytes()).unwrap();
    file.lock().unwrap();
    file
}

fn output_text(out: &Output) -> String {
    format!(
        "status {}\nstdout {}\nstderr {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

impl World {
    fn start() -> Self {
        // The session socket lives under the real HOME. A private HOME is too
        // long for sun_path and has no Claude login. The projects root stays
        // under /tmp, not under ~/.herdr-projects.
        let user_home = PathBuf::from(std::env::var("HOME").expect("HOME"));
        let files = tempfile::Builder::new()
            .prefix("hpo-gap-")
            .tempdir_in("/tmp")
            .unwrap();
        let herdr = real_herdr();
        let server_log = files.path().join("server.log");
        let log_file = File::create(&server_log).unwrap();
        let child = herdr_client(&herdr)
            .args(["--session", SESSION, "server"])
            .stdin(Stdio::null())
            .stdout(log_file.try_clone().unwrap())
            .stderr(log_file)
            .spawn()
            .unwrap();
        let server = Server {
            herdr: herdr.clone(),
            child,
        };
        let socket = wait_socket(&herdr, &server_log);
        let root = files.path().join("root");
        let repo = files.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "T"]);
        git(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let repo = fs::canonicalize(&repo).unwrap();
        let standin = files.path().join("herdr-standin");
        fs::copy(STANDIN, &standin).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&standin).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&standin, perms).unwrap();
        }
        let mode = files.path().join("mode");
        fs::write(&mode, "setup\n").unwrap();
        let state = files.path().join("state.json");
        let log = files.path().join("herdr.log");
        let marker = files.path().join("worktree-created");
        let focus_marker = files.path().join("focus.json");
        let world = World {
            server,
            lock: File::open("/dev/null").unwrap(),
            files,
            user_home,
            extra: ExtraWorktrees {
                repo: repo.clone(),
                herdr: herdr.clone(),
            },
            root: root.clone(),
            repo,
            project_dir: PathBuf::new(),
            socket,
            herdr,
            standin,
            mode,
            state,
            log,
            marker,
            focus_marker,
        };
        let created = world.hp(&[
            "--root",
            world.root.to_str().unwrap(),
            "new",
            "Accept",
            "--repo",
            world.repo.to_str().unwrap(),
        ]);
        assert!(created.status.success(), "{}", output_text(&created));
        let project_dir = fs::canonicalize(world.root.join("accept")).unwrap();
        let version = version(&world.user_home);
        let lock = hold_ticker(&world.root, &version);
        let world = World {
            lock,
            project_dir,
            ..world
        };
        let _ = server_log;
        open_coordinator(&world);
        world
    }

    fn set_mode(&self, mode: &str) {
        fs::write(&self.mode, format!("{mode}\n")).unwrap();
    }

    fn envs(&self, cmd: &mut Command) {
        cmd.env_clear()
            .env("HOME", &self.user_home)
            .env("PATH", PATH)
            .env("HERDR_BIN_PATH", &self.standin)
            .env("HPO_REAL_HERDR", &self.herdr)
            .env("HPO_HERDR_LOG", &self.log)
            .env("HPO_HERDR_MODE", &self.mode)
            .env("HPO_HERDR_STATE", &self.state)
            .env("HPO_WT_MARKER", &self.marker)
            .env("HPO_FOCUS_MARKER", &self.focus_marker)
            .env("HPO_PROJECT_DIR", &self.project_dir);
    }

    fn hp(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(BIN);
        self.envs(&mut cmd);
        cmd.args(args).output().unwrap()
    }

    fn spawn_hp(&self, args: &[&str], out: &Path, err: &Path) -> Child {
        let mut cmd = Command::new(BIN);
        self.envs(&mut cmd);
        cmd.args(args)
            .stdout(File::create(out).unwrap())
            .stderr(File::create(err).unwrap())
            .spawn()
            .unwrap()
    }

    fn prompt_count(&self) -> usize {
        log_lines(&self.log)
            .into_iter()
            .filter(|line| line["argv"][0] == "agent" && line["argv"][1] == "prompt")
            .count()
    }

    fn bound_pane(&self) -> String {
        let text = fs::read_to_string(&self.state).unwrap_or_default();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::json!({}));
        value["pane_id"].as_str().unwrap_or("").to_string()
    }

    fn real(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(&self.herdr);
        cmd.env_clear()
            .env("HOME", &self.user_home)
            .env("PATH", PATH)
            .env("HERDR_SOCKET_PATH", &self.socket);
        cmd.args(args).output().unwrap()
    }
}

struct ExtraWorktrees {
    repo: PathBuf,
    herdr: PathBuf,
}

impl Drop for ExtraWorktrees {
    fn drop(&mut self) {
        let _ = herdr_client(&self.herdr)
            .args(["session", "stop", SESSION])
            .output();
        let listed = Command::new("git")
            .env("PATH", PATH)
            .args([
                "-C",
                self.repo.to_str().unwrap_or("."),
                "worktree",
                "list",
                "--porcelain",
            ])
            .output();
        let Ok(listed) = listed else {
            return;
        };
        let text = String::from_utf8_lossy(&listed.stdout);
        let main = fs::canonicalize(&self.repo).unwrap_or_else(|_| self.repo.clone());
        for line in text.lines() {
            let Some(path) = line.strip_prefix("worktree ") else {
                continue;
            };
            let path = PathBuf::from(path);
            if fs::canonicalize(&path).unwrap_or(path.clone()) == main {
                continue;
            }
            let _ = Command::new("git")
                .env("PATH", PATH)
                .args([
                    "-C",
                    self.repo.to_str().unwrap_or("."),
                    "worktree",
                    "remove",
                    "--force",
                    path.to_str().unwrap_or(""),
                ])
                .status();
        }
    }
}

fn agent_status(world: &World, pane: &str) -> String {
    let info = world.real(&["agent", "get", pane]);
    let text = String::from_utf8_lossy(&info.stdout);
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return String::new();
    };
    value["result"]["agent"]["agent_status"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// Starts Claude in `pane` and accepts the folder-trust dialog until Herdr
/// reports `idle`. The dialog's first choice is "No, exit".
fn answer_trust_dialog(world: &World, pane: &str) -> String {
    let read = world.real(&[
        "agent",
        "read",
        pane,
        "--source",
        "recent-unwrapped",
        "--lines",
        "80",
    ]);
    let mut text = String::from_utf8_lossy(&read.stdout).into_owned();
    if text.trim().is_empty() {
        text = String::from_utf8_lossy(&read.stderr).into_owned();
    }
    if text.contains("Yes, I trust this folder")
        || text.contains("Yes, I accept")
        || text.contains("Quick safety check")
    {
        let _ = world.real(&["agent", "send-keys", pane, "down"]);
        let _ = world.real(&["agent", "send-keys", pane, "enter"]);
    }
    text
}

fn agent_starts(world: &World) -> Vec<serde_json::Value> {
    log_lines(&world.log)
        .into_iter()
        .filter(|line| line["argv"][0] == "agent" && line["argv"][1] == "start")
        .collect()
}

fn open_coordinator(world: &World) {
    world.set_mode("live");
    let out = world.files.path().join("open.out");
    let err = world.files.path().join("open.err");
    let mut child = world.spawn_hp(
        &[
            "--root",
            world.root.to_str().unwrap(),
            "open",
            "accept",
            "--socket",
            world.socket.to_str().unwrap(),
        ],
        &out,
        &err,
    );
    let deadline = Instant::now() + Duration::from_secs(150);
    let mut pane = String::new();
    loop {
        if pane.is_empty() {
            pane = world.bound_pane();
        } else {
            answer_trust_dialog(world, &pane);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = fs::read_to_string(&out).unwrap_or_default();
                let stderr = fs::read_to_string(&err).unwrap_or_default();
                assert!(status.success(), "open failed\n{stdout}\n{stderr}");
                break;
            }
            Ok(None) => {
                assert!(
                    Instant::now() < deadline,
                    "open did not finish\n{}",
                    fs::read_to_string(&world.log).unwrap_or_default()
                );
                thread::sleep(Duration::from_millis(300));
            }
            Err(error) => panic!("open wait failed: {error}"),
        }
    }
    assert!(!pane.is_empty(), "open did not record a herdr pane");
    let idle_deadline = Instant::now() + Duration::from_secs(60);
    let mut last_status;
    let mut last_screen;
    loop {
        last_screen = answer_trust_dialog(world, &pane);
        last_status = agent_status(world, &pane);
        if last_status == "idle" {
            break;
        }
        assert!(
            Instant::now() < idle_deadline,
            "coordinator on {pane} did not become idle after open\nstatus {last_status}\n{last_screen}\n{}",
            fs::read_to_string(&world.log).unwrap_or_default()
        );
        thread::sleep(Duration::from_secs(1));
    }
    let starts = agent_starts(world);
    assert!(!starts.is_empty(), "open did not start an agent");
    assert!(
        starts.iter().all(|line| line["delegated"] == true),
        "agent start did not go through herdr\n{starts:?}"
    );
    println!(
        "OPEN coordinator pane {pane} status idle starts {}",
        starts.len()
    );
    world.set_mode("idle");
}

fn bring_up(world: &World, pane: &str, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(90);
    let start_text = loop {
        assert!(
            Instant::now() < deadline,
            "agent {name} on {pane} stayed busy"
        );
        let started = world.real(&[
            "agent",
            "start",
            name,
            "--kind",
            "claude",
            "--pane",
            pane,
            "--timeout",
            "90000",
            "--",
            "--dangerously-skip-permissions",
        ]);
        let start_text = output_text(&started);
        if start_text.contains("agent_pane_busy") {
            thread::sleep(Duration::from_millis(500));
            continue;
        }
        break start_text;
    };
    loop {
        let read = world.real(&[
            "agent",
            "read",
            pane,
            "--source",
            "recent-unwrapped",
            "--lines",
            "80",
        ]);
        let text = String::from_utf8_lossy(&read.stdout).to_string();
        if Instant::now() >= deadline {
            panic!("agent {name} on {pane} did not become idle\n{start_text}\n{text}");
        }
        if text.contains("Yes, I trust this folder") || text.contains("Yes, I accept") {
            let _ = world.real(&["agent", "send-keys", pane, "down"]);
            let _ = world.real(&["agent", "send-keys", pane, "enter"]);
            thread::sleep(Duration::from_secs(2));
            continue;
        }
        if agent_status(world, pane) == "idle" {
            return;
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .env("PATH", PATH)
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}\n{}", output_text(&out));
}

fn wait_socket(herdr: &Path, server_log: &Path) -> PathBuf {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let out = herdr_client(herdr)
            .args(["session", "list", "--json"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        if let Some(path) = socket_from(&text) {
            let socket = PathBuf::from(path);
            assert!(
                socket.as_os_str().len() < 104,
                "socket path is too long for sun_path: {}",
                socket.display()
            );
            return socket;
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "session did not start\n{}\n{}",
        fs::read_to_string(server_log).unwrap_or_default(),
        "no socket"
    );
}

fn socket_from(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let sessions = value
        .get("sessions")
        .or_else(|| value.get("result").and_then(|item| item.get("sessions")))?
        .as_array()?;
    for session in sessions {
        let name = session
            .get("name")
            .and_then(|item| item.as_str())
            .unwrap_or("");
        if name == SESSION {
            return session
                .get("socket_path")
                .and_then(|item| item.as_str())
                .map(str::to_string);
        }
    }
    None
}

fn log_lines(path: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn squashed(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if !ch.is_whitespace() {
            out.push(ch);
        }
    }
    out
}

fn drive(world: &World, name: &str, steps: serde_json::Value) -> (String, u32) {
    let dir = world.files.path().join(name);
    fs::create_dir_all(&dir).unwrap();
    let script = dir.join("steps.json");
    let capture = dir.join("capture.bin");
    let pid_path = dir.join("pid");
    fs::write(&script, serde_json::to_vec(&steps).unwrap()).unwrap();
    let mut cmd = Command::new("/usr/bin/python3");
    world.envs(&mut cmd);
    cmd.arg(DRIVE)
        .arg("drive")
        .arg("--rows")
        .arg("40")
        .arg("--cols")
        .arg("160")
        .arg("--script")
        .arg(&script)
        .arg("--capture")
        .arg(&capture)
        .arg("--pid")
        .arg(&pid_path)
        .arg("--")
        .arg(BIN)
        .arg("--root")
        .arg(&world.root)
        .arg("popup")
        .arg("--next")
        .arg("accept");
    let mut child = cmd.spawn().unwrap();
    let started = Instant::now();
    let pid = wait_pid(&pid_path, started);
    let status = child.wait().unwrap();
    let bytes = fs::read(&capture).unwrap_or_default();
    keep(name, &bytes);
    assert!(
        status.success(),
        "{name} driver failed\n{}",
        squashed(&bytes)
    );
    (squashed(&bytes), pid)
}

fn drive_until_marker(world: &World) -> u32 {
    let dir = world.files.path().join("crash");
    fs::create_dir_all(&dir).unwrap();
    let script = dir.join("steps.json");
    let capture = dir.join("capture.bin");
    let pid_path = dir.join("pid");
    let steps = serde_json::json!([
        {"wait": "HERDRPROJECTS", "timeout": 8},
        {"send": "g"},
        {"send": "1"},
        {"send": "t"},
        {"wait": "Kindworktree", "timeout": 3},
        {"send": "gap"},
        {"send": "\t"},
        {"send": "do"},
        {"send": "\t"},
        {"send": world.repo.to_str().unwrap()},
        {"send": "\r"},
        {"wait_file": world.marker.to_str().unwrap(), "timeout": 20},
        {"hold": 25}
    ]);
    fs::write(&script, serde_json::to_vec(&steps).unwrap()).unwrap();
    let mut cmd = Command::new("/usr/bin/python3");
    world.envs(&mut cmd);
    cmd.arg(DRIVE)
        .arg("drive")
        .arg("--rows")
        .arg("40")
        .arg("--cols")
        .arg("160")
        .arg("--script")
        .arg(&script)
        .arg("--capture")
        .arg(&capture)
        .arg("--pid")
        .arg(&pid_path)
        .arg("--")
        .arg(BIN)
        .arg("--root")
        .arg(&world.root)
        .arg("popup")
        .arg("--next")
        .arg("accept");
    let mut child = cmd.spawn().unwrap();
    let started = Instant::now();
    let pid = wait_pid(&pid_path, started);
    while !world.marker.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(25),
            "worktree marker missing\n{}",
            fs::read_to_string(&world.log).unwrap_or_default()
        );
        thread::sleep(Duration::from_millis(50));
    }
    let args = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output()
        .unwrap();
    let args = String::from_utf8_lossy(&args.stdout);
    assert!(args.contains("popup"), "pid {pid} is not the TUI: {args}");
    let killed = Command::new("/bin/kill")
        .args(["-KILL", &format!("-{pid}")])
        .status()
        .unwrap();
    assert!(killed.success(), "kill of the TUI group failed");
    let _ = child.wait();
    let bytes = fs::read(&capture).unwrap_or_default();
    keep("crash-killed", &bytes);
    let alive = Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .output()
        .unwrap();
    assert!(!alive.status.success(), "TUI {pid} still exists");
    pid
}

fn wait_pid(path: &Path, started: Instant) -> u32 {
    loop {
        if let Ok(text) = fs::read_to_string(path)
            && let Ok(pid) = text.trim().parse::<u32>()
        {
            return pid;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "pty pid missing"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn keep(name: &str, bytes: &[u8]) {
    let Ok(dir) = std::env::var("HPO_EVIDENCE") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let _ = fs::create_dir_all(&dir);
    let _ = fs::write(dir.join(format!("{name}.txt")), squashed(bytes));
}

fn inspect(world: &World) -> serde_json::Value {
    let db = world.root.join("registry.sqlite");
    let out = Command::new("/usr/bin/python3")
        .args([DRIVE, "inspect", db.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", output_text(&out));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn prompt_case(world: &World, mode: &str, needle: &str, again: bool) -> String {
    world.set_mode(mode);
    let before = world.prompt_count();
    let mut steps = vec![
        serde_json::json!({"wait": "HERDRPROJECTS", "timeout": 8}),
        serde_json::json!({"send": "\u{1b}[B"}),
        serde_json::json!({"send": "\r"}),
        serde_json::json!({"send": "hello"}),
        serde_json::json!({"send": "\r"}),
        serde_json::json!({"wait": needle, "timeout": 8}),
    ];
    if again {
        steps.push(serde_json::json!({"send": "\r"}));
        steps.push(serde_json::json!({"hold": 1}));
    }
    steps.push(serde_json::json!({"send": "\u{1b}"}));
    steps.push(serde_json::json!({"hold": 0.2}));
    steps.push(serde_json::json!({"send": "\u{1b}"}));
    steps.push(serde_json::json!({"hold": 1}));
    let (text, _) = drive(world, mode, serde_json::Value::Array(steps));
    let after = world.prompt_count();
    println!("PROMPT {mode} calls {before}->{after} needle {needle}");
    text
}

#[test]
fn live_crash_window_open_worker_and_prompt_outcomes() {
    let world = World::start();
    let _ = world.server.child.id();
    let _ = &world.lock;
    let _ = &world.extra;
    let pane = world.bound_pane();
    assert!(!pane.is_empty(), "open did not record a herdr pane");

    let missing = prompt_case(&world, "missing", "Refused", false);
    assert!(missing.contains("hello"), "{missing}");
    assert_eq!(world.prompt_count(), 0, "missing coordinator sent a prompt");

    let blocked = prompt_case(&world, "blocked", "Refused", false);
    assert!(blocked.contains("hello"), "{blocked}");
    assert_eq!(world.prompt_count(), 0, "blocked coordinator sent a prompt");

    let unknown = prompt_case(&world, "unknown", "Refused", false);
    assert!(unknown.contains("hello"), "{unknown}");
    assert_eq!(world.prompt_count(), 0, "unknown coordinator sent a prompt");

    let before_refused = world.prompt_count();
    let refused = prompt_case(&world, "refused", "Refused", false);
    assert!(refused.contains("hello"), "{refused}");
    assert_eq!(world.prompt_count(), before_refused + 1);
    thread::sleep(Duration::from_millis(400));
    assert_eq!(
        world.prompt_count(),
        before_refused + 1,
        "refused prompt was sent again with no key"
    );

    let before_uncertain = world.prompt_count();
    let uncertain = prompt_case(&world, "uncertain", "Uncertain", true);
    assert!(uncertain.contains("notsentagain"), "{uncertain}");
    assert!(uncertain.contains("hello"), "{uncertain}");
    assert_eq!(
        world.prompt_count(),
        before_uncertain + 1,
        "uncertain prompt was sent again"
    );
    assert!(!uncertain.contains("replied"), "{uncertain}");

    let starts_before_prompt = agent_starts(&world).len();
    let before_live = world.prompt_count();
    let live = prompt_case(&world, "live", "Confirmed", false);
    assert!(!live.contains("replied"), "{live}");
    assert_eq!(world.prompt_count(), before_live + 1);
    let prompts: Vec<_> = log_lines(&world.log)
        .into_iter()
        .filter(|line| line["argv"][0] == "agent" && line["argv"][1] == "prompt")
        .collect();
    let last = prompts.last().unwrap();
    assert_eq!(last["delegated"], true, "{last}");
    assert_eq!(last["exit"], 0, "{last}");
    assert_eq!(last["argv"][2], pane);
    assert!(
        last["argv"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == "--wait")
    );
    assert!(
        last["argv"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item != "w9:p9")
    );
    let observed: serde_json::Value = serde_json::from_str(last["out"].as_str().unwrap())
        .unwrap_or_else(|error| panic!("{error} {}", last["out"]));
    let status = observed["result"]["agent"]["agent_status"]
        .as_str()
        .unwrap_or("");
    assert!(
        status == "working" || status == "blocked",
        "herdr did not observe the bound agent\n{observed}"
    );
    println!("OBSERVED status {status} pane {pane}");
    assert_eq!(
        agent_starts(&world).len(),
        starts_before_prompt,
        "confirmed prompt started another agent"
    );

    world.set_mode("idle");
    let task = world.files.path().join("task.md");
    fs::write(&task, "Do the work.\n").unwrap();
    let started = world.hp(&[
        "--root",
        world.root.to_str().unwrap(),
        "thread",
        "start",
        "accept",
        "--title",
        "Worker",
        "--kind",
        "tab",
        "--task-file",
        task.to_str().unwrap(),
    ]);
    assert!(started.status.success(), "{}", output_text(&started));
    let rows = inspect(&world);
    let worker = rows["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["title"] == "Worker")
        .unwrap();
    let worker_pane = worker["pane_id"].as_str().unwrap();
    assert!(!worker_pane.is_empty(), "{worker}");
    bring_up(&world, worker_pane, "hpc-worker");
    let _ = fs::remove_file(&world.focus_marker);
    let (opened, _) = drive(
        &world,
        "open-worker",
        serde_json::json!([
            {"wait": "HERDRPROJECTS", "timeout": 8},
            {"send": "g"},
            {"send": "1"},
            {"send": "o"},
            {"wait_file": world.focus_marker.to_str().unwrap(), "timeout": 8},
            {"hold": 1},
            {"send": "\u{1b}"}
        ]),
    );
    let focus: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&world.focus_marker).unwrap()).unwrap();
    assert_eq!(focus["argv"][2], worker_pane, "{focus}");
    assert_eq!(focus["exit"], 0, "{focus}");
    let delegated = log_lines(&world.log)
        .into_iter()
        .find(|line| {
            line["argv"][0] == "agent"
                && line["argv"][1] == "focus"
                && line["argv"][2] == worker_pane
                && line["delegated"] == true
        })
        .expect("agent focus was not delegated to herdr");
    println!(
        "OPEN o pane {worker_pane} exit {} delegated {} screen {}",
        focus["exit"],
        delegated["delegated"],
        opened.contains("focused")
    );

    world.set_mode("idle");
    finish_worktree(&world);
    world.set_mode("hold");
    let _ = fs::remove_file(&world.marker);
    let killed = drive_until_marker(&world);
    println!("KILLED tui pid {killed}");
    let rows = inspect(&world);
    let gap = rows["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["title"] == "gap")
        .unwrap_or_else(|| panic!("{rows}"));
    assert_eq!(gap["worktree_path"], "");
    assert_eq!(gap["branch"], "");
    assert_eq!(gap["kind"], "worktree");
    let op = rows["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            row["payload"]
                .as_str()
                .unwrap_or("")
                .contains(gap["id"].as_str().unwrap())
                && row["status"] == "pending"
        })
        .unwrap_or_else(|| panic!("{rows}"));
    assert_eq!(op["status"], "pending");
    let listed = Command::new("git")
        .env("PATH", PATH)
        .args([
            "-C",
            world.repo.to_str().unwrap(),
            "worktree",
            "list",
            "--porcelain",
        ])
        .output()
        .unwrap();
    assert!(listed.status.success(), "{}", output_text(&listed));
    let listed = String::from_utf8(listed.stdout).unwrap();
    assert!(
        listed
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count()
            >= 2,
        "{listed}"
    );
    let gap_id = gap["id"].as_str().unwrap().to_string();
    let create = log_lines(&world.log)
        .into_iter()
        .find(|line| {
            line["argv"][0] == "worktree"
                && line["argv"][1] == "create"
                && line["delegated"] == true
                && line["argv"].as_array().is_some_and(|argv| {
                    argv.iter()
                        .any(|item| item.as_str().unwrap_or("").contains(gap_id.as_str()))
                })
        })
        .unwrap_or_else(|| panic!("worktree create for {gap_id} was not delegated"));
    let branch = create["argv"]
        .as_array()
        .unwrap()
        .windows(2)
        .find(|pair| pair[0] == "--branch")
        .unwrap()[1]
        .as_str()
        .unwrap()
        .to_string();
    let branches = Command::new("git")
        .env("PATH", PATH)
        .args([
            "-C",
            world.repo.to_str().unwrap(),
            "branch",
            "--list",
            &branch,
        ])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&branches.stdout).contains(&branch),
        "branch {branch} missing\n{}",
        String::from_utf8_lossy(&branches.stdout)
    );
    let trees_before = worktree_paths(&world.repo);
    let (restarted, _) = drive(
        &world,
        "restart",
        serde_json::json!([
            {"wait": "unresolvedrecovery", "timeout": 8},
            {"wait": "nothingwasattached", "timeout": 3},
            {"send": "\u{1b}"}
        ]),
    );
    assert!(restarted.contains("unresolvedrecovery"), "{restarted}");
    assert!(restarted.contains("nothingwasattached"), "{restarted}");
    let again = inspect(&world);
    let gap_again = again["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["title"] == "gap")
        .unwrap();
    assert_eq!(gap_again["worktree_path"], "");
    assert_eq!(gap_again["id"], gap["id"]);
    assert_eq!(
        again["threads"].as_array().unwrap().len(),
        rows["threads"].as_array().unwrap().len()
    );
    assert_eq!(
        worktree_paths(&world.repo),
        trees_before,
        "restart created another worktree"
    );
    println!(
        "CRASH id {} op {} branch {branch} attached false worktrees {}",
        gap["id"],
        op["id"],
        trees_before.len()
    );
}

fn finish_worktree(world: &World) {
    let before_threads = inspect(world)["threads"].as_array().unwrap().len();
    let before_trees = worktree_paths(&world.repo);
    let task = world.files.path().join("wt-task.md");
    fs::write(&task, "Do the work.\n").unwrap();
    let started = world.hp(&[
        "--root",
        world.root.to_str().unwrap(),
        "thread",
        "start",
        "accept",
        "--title",
        "Finished",
        "--kind",
        "worktree",
        "--repo",
        world.repo.to_str().unwrap(),
        "--task-file",
        task.to_str().unwrap(),
    ]);
    assert!(started.status.success(), "{}", output_text(&started));
    let rows = inspect(world);
    let threads = rows["threads"].as_array().unwrap();
    assert_eq!(threads.len(), before_threads + 1);
    let thread = threads
        .iter()
        .find(|row| row["title"] == "Finished")
        .unwrap_or_else(|| panic!("{rows}"));
    let path = thread["worktree_path"].as_str().unwrap();
    assert!(!path.is_empty(), "{thread}");
    assert!(Path::new(path).is_dir(), "{path}");
    let trees = worktree_paths(&world.repo);
    assert_eq!(trees.len(), before_trees.len() + 1, "{trees:?}");
    assert_eq!(
        threads
            .iter()
            .filter(|row| row["title"] == "Finished")
            .count(),
        1
    );
    println!(
        "WORKTREE id {} path {path} trees {}",
        thread["id"],
        trees.len()
    );
}

fn worktree_paths(repo: &Path) -> Vec<String> {
    let listed = Command::new("git")
        .env("PATH", PATH)
        .args([
            "-C",
            repo.to_str().unwrap(),
            "worktree",
            "list",
            "--porcelain",
        ])
        .output()
        .unwrap();
    assert!(listed.status.success(), "{}", output_text(&listed));
    String::from_utf8(listed.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(str::to_string)
        .collect()
}
