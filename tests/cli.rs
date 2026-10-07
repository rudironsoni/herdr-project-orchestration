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
    assert!(hp(home.path(), &["--root", root_arg, "new", "Demo"]).status.success());

    let out = hp(home.path(), &["--root", root_arg, "context", "demo", "--peek"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    let prefix = text.lines().next().unwrap().strip_prefix("Commands: ").unwrap();
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
    assert_eq!(String::from_utf8_lossy(&listed.stdout), "demo\tactive\tno threads\n");
}

#[test]
fn peek_records_nothing_and_context_records_seen_items() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(hp(home.path(), &["--root", root_arg, "new", "demo"]).status.success());
    let item = "+++\nid = \"20260917T000000Z-routine-r-1\"\nkind = \"routine\"\nsubject = \"r\"\ncreated = \"x\"\nsummary = \"s\"\n+++\n";
    std::fs::write(root.join("demo/inbox/20260917T000000Z-routine-r-1.md"), item).unwrap();
    let seen = root.join("demo/.state/inbox-seen.json");

    assert!(hp(home.path(), &["--root", root_arg, "context", "demo", "--peek"]).status.success());
    assert!(!seen.exists());
    assert!(hp(home.path(), &["--root", root_arg, "context", "demo"]).status.success());
    assert!(std::fs::read_to_string(&seen).unwrap().contains("routine-r-1"));
}

#[test]
fn path_like_names_and_slugs_are_refused() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(!hp(home.path(), &["--root", root_arg, "new", "../x"]).status.success());
    assert!(!hp(home.path(), &["--root", root_arg, "open", "../x"]).status.success());
    assert!(!hp(home.path(), &["--root", root_arg, "context", "../x"]).status.success());
    assert!(!hp(home.path(), &["--root", root_arg, "thread", "list", "../x"]).status.success());
    assert!(!hp(home.path(), &["--root", root_arg, "delete", "../x", "--force"]).status.success());
    assert!(!root.exists());
    assert!(!home.path().join("x").exists());
}

#[test]
fn pause_ignores_a_lying_project_json_and_doctor_names_the_registry() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(hp(home.path(), &["--root", root_arg, "new", "demo"]).status.success());
    assert!(hp(home.path(), &["--root", root_arg, "pause", "demo"]).status.success());
    let legacy = root.join("demo/.state/project.json");
    assert!(!legacy.exists(), "pause must not write project.json");
    std::fs::write(&legacy, "{\"status\":\"archived\",\"former_slugs\":[\"bogus\"]}\n").unwrap();
    let listed = hp(home.path(), &["--root", root_arg, "list"]);
    assert!(listed.status.success(), "{}", String::from_utf8_lossy(&listed.stderr));
    assert_eq!(String::from_utf8_lossy(&listed.stdout), "demo\tpaused\tno threads\n");
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
        handles.push(std::thread::spawn(move || hp(&home, &["--root", &root_arg, "list"])));
    }
    for handle in handles {
        let output = handle.join().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "demo\tpaused\tno threads\n");
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
    assert!(listed.status.success(), "{}", String::from_utf8_lossy(&listed.stderr));
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        "demo\timport-failed\tsee doctor\n"
    );
    std::fs::write(&legacy, "{\"status\":\"paused\",\"former_slugs\":[\"old\"]}\n").unwrap();
    let retry = hp(home.path(), &["--root", root_arg, "migrate", "retry"]);
    assert!(retry.status.success(), "{}", String::from_utf8_lossy(&retry.stderr));
    let listed = hp(home.path(), &["--root", root_arg, "list"]);
    assert_eq!(String::from_utf8_lossy(&listed.stdout), "demo\tpaused\tno threads\n");
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
        handles.push(std::thread::spawn(move || hp(&home, &["--root", &root_arg, "new", "demo"])));
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
    let projects: i64 = database.query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0)).unwrap();
    assert_eq!(projects, 1);
}

#[test]
fn ticker_start_without_projects_creates_nothing() {
    let home = tempfile::tempdir().unwrap();
    assert!(hp(home.path(), &["ticker", "start"]).status.success());
    assert!(!home.path().join(".herdr-projects").exists());
    assert!(!home.path().join(".config").exists());
}
