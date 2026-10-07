//! End-to-end checks of the built binary with a scrubbed environment.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_herdr-projects");

fn hp(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .env_clear()
        .env("HOME", home)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn context_prints_a_usable_prefix_in_a_scrubbed_environment() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("my root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "Demo"])
            .status
            .success()
    );

    let out = hp(
        home.path(),
        &["--root", root_arg, "context", "demo", "--peek"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    let prefix = text
        .lines()
        .next()
        .unwrap()
        .strip_prefix("Commands: ")
        .unwrap();
    let binary = std::fs::canonicalize(BIN).unwrap_or_else(|_| std::path::PathBuf::from(BIN));
    assert_eq!(prefix, format!("{} --root '{root_arg}'", binary.display()));

    // The printed prefix works as typed, from a bare shell.
    let listed = Command::new("/bin/sh")
        .env_clear()
        .env("HOME", home.path())
        .args(["-c", &format!("{prefix} list")])
        .output()
        .unwrap();
    assert!(listed.status.success());
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        "demo\tactive\tno threads\n"
    );
}

#[test]
fn peek_records_nothing_and_context_records_seen_items() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "demo"])
            .status
            .success()
    );
    let item = "+++\nid = \"20260917T000000Z-routine-r-1\"\nkind = \"routine\"\nsubject = \"r\"\ncreated = \"x\"\nsummary = \"s\"\n+++\n";
    std::fs::write(
        root.join("demo/inbox/20260917T000000Z-routine-r-1.md"),
        item,
    )
    .unwrap();
    let seen = root.join("demo/.state/inbox-seen.json");

    assert!(
        hp(
            home.path(),
            &["--root", root_arg, "context", "demo", "--peek"]
        )
        .status
        .success()
    );
    assert!(!seen.exists());
    assert!(
        hp(home.path(), &["--root", root_arg, "context", "demo"])
            .status
            .success()
    );
    assert!(
        std::fs::read_to_string(&seen)
            .unwrap()
            .contains("routine-r-1")
    );
}

#[test]
fn path_like_names_and_slugs_are_refused() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        !hp(home.path(), &["--root", root_arg, "new", "../x"])
            .status
            .success()
    );
    assert!(
        !hp(home.path(), &["--root", root_arg, "open", "../x"])
            .status
            .success()
    );
    assert!(
        !hp(home.path(), &["--root", root_arg, "context", "../x"])
            .status
            .success()
    );
    assert!(
        !hp(home.path(), &["--root", root_arg, "thread", "list", "../x"])
            .status
            .success()
    );
    assert!(
        !hp(
            home.path(),
            &["--root", root_arg, "delete", "../x", "--force"]
        )
        .status
        .success()
    );
    assert!(!root.exists());
    assert!(!home.path().join("x").exists());
}

#[test]
fn pause_ignores_a_lying_project_json_and_doctor_names_the_registry() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "demo"])
            .status
            .success()
    );
    assert!(
        hp(home.path(), &["--root", root_arg, "pause", "demo"])
            .status
            .success()
    );
    let legacy = root.join("demo/.state/project.json");
    assert!(!legacy.exists(), "pause must not write project.json");
    std::fs::write(
        &legacy,
        "{\"status\":\"archived\",\"former_slugs\":[\"bogus\"]}\n",
    )
    .unwrap();
    let listed = hp(home.path(), &["--root", root_arg, "list"]);
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        "demo\tpaused\tno threads\n"
    );
    let doctor = hp(home.path(), &["--root", root_arg, "doctor"]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&doctor.stdout),
        String::from_utf8_lossy(&doctor.stderr)
    );
    assert!(
        text.contains("do not use herdr-projects 0.2.34 against this root"),
        "{text}"
    );
}

#[test]
fn concurrent_list_imports_a_legacy_project_once() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap().to_string();
    std::fs::create_dir_all(root.join("demo/.state")).unwrap();
    std::fs::write(root.join("demo/PROJECT.md"), "+++\nname = \"Demo\"\n+++\n").unwrap();
    std::fs::write(
        root.join("demo/.state/project.json"),
        "{\"status\":\"paused\",\"former_slugs\":[\"old\"]}\n",
    )
    .unwrap();
    let mut handles = Vec::new();
    for _ in 0..4 {
        let home = home.path().to_path_buf();
        let root_arg = root_arg.clone();
        handles.push(std::thread::spawn(move || {
            hp(&home, &["--root", &root_arg, "list"])
        }));
    }
    for handle in handles {
        let output = handle.join().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "demo\tpaused\tno threads\n"
        );
    }
    let database = rusqlite::Connection::open(root.join("registry.sqlite")).unwrap();
    let projects: i64 = database
        .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
        .unwrap();
    let imports: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM legacy_imports WHERE status = 'imported'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let check: String = database
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(projects, 1);
    assert_eq!(imports, 1);
    assert_eq!(check, "ok");
}

#[test]
fn migrate_retry_imports_a_repaired_project_json() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    std::fs::create_dir_all(root.join("demo/.state")).unwrap();
    std::fs::write(root.join("demo/PROJECT.md"), "+++\nname = \"Demo\"\n+++\n").unwrap();
    let legacy = root.join("demo/.state/project.json");
    std::fs::write(&legacy, "{").unwrap();
    let listed = hp(home.path(), &["--root", root_arg, "list"]);
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        "demo\timport-failed\tsee doctor\n"
    );
    std::fs::write(
        &legacy,
        "{\"status\":\"paused\",\"former_slugs\":[\"old\"]}\n",
    )
    .unwrap();
    let retry = hp(home.path(), &["--root", root_arg, "migrate", "retry"]);
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    let listed = hp(home.path(), &["--root", root_arg, "list"]);
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        "demo\tpaused\tno threads\n"
    );
}

#[test]
fn two_processes_cannot_create_the_same_project() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap().to_string();
    let mut handles = Vec::new();
    for _ in 0..2 {
        let home = home.path().to_path_buf();
        let root_arg = root_arg.clone();
        handles.push(std::thread::spawn(move || {
            hp(&home, &["--root", &root_arg, "new", "demo"])
        }));
    }
    let mut created = 0;
    let mut refused = 0;
    for handle in handles {
        if handle.join().unwrap().status.success() {
            created += 1;
        } else {
            refused += 1;
        }
    }
    assert!(created >= 1, "refused {refused}");
    let database = rusqlite::Connection::open(root.join("registry.sqlite")).unwrap();
    let projects: i64 = database
        .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
        .unwrap();
    assert_eq!(projects, 1);
}

fn write_stub_herdr(home: &Path) -> std::path::PathBuf {
    let path = home.join("herdr");
    std::fs::write(
        &path,
        r#"#!/bin/sh
args="$*"
cwd="${STUB_CWD:-/tmp}"
case "$args" in
  *"machine list"*)
    printf '%s\n' '[{"id":"mach_1","label":"box","target":"box.example"}]'
    ;;
  *"agent list"*)
    printf '{"result":{"agents":[{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","name":"a","agent":"claude","agent_status":"idle","cwd":"%s"},{"pane_id":"w1:p2","tab_id":"w1:t1","workspace_id":"w1","name":"b","agent":"claude","agent_status":"idle","cwd":"%s"}]}}\n' "$cwd" "$cwd"
    ;;
  *"pane list"*)
    printf '{"result":{"panes":[{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","cwd":"%s"},{"pane_id":"w1:p2","tab_id":"w1:t1","workspace_id":"w1","cwd":"%s"}]}}\n' "$cwd" "$cwd"
    ;;
  *"workspace create"*|*"tab create"*)
    printf '%s\n' '{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p9"}}}'
    ;;
  *"workspace get"*)
    printf '%s\n' '{"result":{"workspace":{"workspace_id":"w1","label":"Demo"}}}'
    ;;
  *)
    printf '%s\n' '{"result":{}}'
    ;;
esac
exit 0
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    path
}

fn hp_herdr(home: &Path, herdr: &Path, cwd: &str, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .env_clear()
        .env("HOME", home)
        .env("HERDR_BIN_PATH", herdr)
        .env("STUB_CWD", cwd)
        .env("PATH", "/usr/bin:/bin")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn separate_processes_do_not_share_a_thread_label_or_a_live_pane() {
    let home = tempfile::tempdir().unwrap();
    let herdr = write_stub_herdr(home.path());
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap().to_string();
    let socket = home.path().join("session.sock");
    std::fs::write(&socket, b"").unwrap();
    let socket_arg = socket.to_str().unwrap().to_string();
    assert!(
        hp(home.path(), &["--root", &root_arg, "new", "demo"])
            .status
            .success()
    );
    assert!(
        hp(home.path(), &["--root", &root_arg, "new", "other"])
            .status
            .success()
    );
    let demo_dir = root.join("demo").canonicalize().unwrap();
    let cwd = demo_dir.to_str().unwrap().to_string();
    let opened = hp_herdr(
        home.path(),
        &herdr,
        &cwd,
        &["--root", &root_arg, "open", "demo", "--socket", &socket_arg],
    );
    assert!(
        opened.status.success(),
        "{}",
        String::from_utf8_lossy(&opened.stderr)
    );

    let task = home.path().join("task.txt");
    std::fs::write(&task, "do the work\n").unwrap();
    let task_arg = task.to_str().unwrap().to_string();
    let mut starts = Vec::new();
    for title in ["one", "two"] {
        let home_path = home.path().to_path_buf();
        let herdr_path = herdr.clone();
        let cwd = cwd.clone();
        let root_arg = root_arg.clone();
        let task_arg = task_arg.clone();
        let title = title.to_string();
        starts.push(std::thread::spawn(move || {
            hp_herdr(
                &home_path,
                &herdr_path,
                &cwd,
                &[
                    "--root",
                    &root_arg,
                    "thread",
                    "start",
                    "demo",
                    "--title",
                    &title,
                    "--kind",
                    "tab",
                    "--task-file",
                    &task_arg,
                ],
            )
        }));
    }
    for handle in starts {
        let output = handle.join().unwrap();
        assert!(
            output.status.success(),
            "stdout {}\nstderr {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let mut binds = Vec::new();
    for (slug, pane) in [("demo", "w1:p1"), ("other", "w1:p1")] {
        let home_path = home.path().to_path_buf();
        let herdr_path = herdr.clone();
        let cwd = cwd.clone();
        let root_arg = root_arg.clone();
        let socket_arg = socket_arg.clone();
        let slug = slug.to_string();
        let pane = pane.to_string();
        binds.push(std::thread::spawn(move || {
            hp_herdr(
                &home_path,
                &herdr_path,
                &cwd,
                &[
                    "--root",
                    &root_arg,
                    "coordinator",
                    "adopt",
                    &slug,
                    "--pane",
                    &pane,
                    "--machine",
                    "box",
                    "--socket",
                    &socket_arg,
                    "--replace-primary",
                ],
            )
        }));
    }
    let mut adopted = 0;
    let mut bind_errors = Vec::new();
    for handle in binds {
        let output = handle.join().unwrap();
        if output.status.success() {
            adopted += 1;
        } else {
            bind_errors.push(format!(
                "stdout {}\nstderr {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    assert_eq!(adopted, 1, "{bind_errors:?}");
    let database = rusqlite::Connection::open(root.join("registry.sqlite")).unwrap();
    let live_pane: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE pane_id = 'w1:p1' AND unbound_at IS NULL AND stale = 0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(live_pane, 1, "{bind_errors:?}");

    let mut primaries = Vec::new();
    for pane in ["w1:p1", "w1:p2"] {
        let home_path = home.path().to_path_buf();
        let herdr_path = herdr.clone();
        let cwd = cwd.clone();
        let root_arg = root_arg.clone();
        let socket_arg = socket_arg.clone();
        let pane = pane.to_string();
        primaries.push(std::thread::spawn(move || {
            hp_herdr(
                &home_path,
                &herdr_path,
                &cwd,
                &[
                    "--root",
                    &root_arg,
                    "coordinator",
                    "adopt",
                    "demo",
                    "--pane",
                    &pane,
                    "--socket",
                    &socket_arg,
                    "--replace-primary",
                ],
            )
        }));
    }
    for handle in primaries {
        let _ = handle.join().unwrap();
    }

    let database = rusqlite::Connection::open(root.join("registry.sqlite")).unwrap();
    let labels: i64 = database
        .query_row(
            "SELECT COUNT(DISTINCT label) FROM threads WHERE label IN ('t-0001', 't-0002')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let dupes: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM (SELECT label FROM threads GROUP BY project_id, label HAVING COUNT(*) > 1)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let primaries_n: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM projects WHERE slug = 'demo' AND primary_session_id IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(labels, 2);
    assert_eq!(dupes, 0);
    assert_eq!(live_pane, 1);
    assert_eq!(primaries_n, 1);
}

#[test]
fn ticker_start_without_projects_creates_nothing() {
    let home = tempfile::tempdir().unwrap();
    assert!(hp(home.path(), &["ticker", "start"]).status.success());
    assert!(!home.path().join(".herdr-projects").exists());
    assert!(!home.path().join(".config").exists());
}
