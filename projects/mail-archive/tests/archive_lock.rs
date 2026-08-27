use mail_archive_experiment::{ArchiveSession, ArchiveWriter};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

fn test_root(name: &str) -> PathBuf {
    let root = PathBuf::from("/var/tmp").join(format!(
        "memoria-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap() == ".memoria-archive.writer.lock" {
            continue;
        }
        if path.is_file() {
            files.push((path.clone(), fs::read(path).unwrap()));
        }
    }
    let archive = root.join("archive");
    if archive.exists() {
        for entry in fs::read_dir(archive).unwrap() {
            let path = entry.unwrap().path();
            files.push((path.clone(), fs::read(path).unwrap()));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

#[test]
fn second_writer_fails_before_archive_mutation_and_reopens_after_drop() {
    let root = test_root("single-writer");
    let mut first = ArchiveSession::create(&root, 4096).unwrap();
    first.writer_mut().append_raw(1, b"first").unwrap();
    first.writer_mut().durable_barrier().unwrap();
    let before = snapshot(&root);

    let error = ArchiveWriter::open(&root.join("archive"), 4096).unwrap_err();
    assert!(error.to_string().contains("ArchiveAlreadyLocked"));
    assert_eq!(snapshot(&root), before);

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("archive"), root.join("archive-alias")).unwrap();
        let error = ArchiveWriter::open(&root.join("archive-alias"), 4096).unwrap_err();
        assert!(error.to_string().contains("ArchiveAlreadyLocked"));
    }

    drop(first);
    let second = ArchiveWriter::open(&root.join("./archive"), 4096).unwrap();
    drop(second);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn independent_archives_have_independent_authority() {
    let root = test_root("independent");
    let a = ArchiveWriter::open(&root.join("a"), 4096).unwrap();
    let b = ArchiveWriter::open(&root.join("b"), 4096).unwrap();
    drop(b);
    drop(a);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn symlink_in_different_parent_converges_on_existing_archive_authority() {
    let root = test_root("cross-parent-symlink");
    let p1 = root.join("p1");
    let p2 = root.join("p2");
    let real = p1.join("archive-root");
    let alias = p2.join("alias");
    fs::create_dir_all(&p1).unwrap();
    fs::create_dir_all(&p2).unwrap();
    std::os::unix::fs::symlink(&real, &alias).unwrap();

    let first = ArchiveSession::create(&real, 4096).unwrap();
    let error = match ArchiveSession::create(&alias, 4096) {
        Ok(_) => panic!("symlink alias unexpectedly acquired the archive authority"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("ArchiveAlreadyLocked"));
    drop(first);

    let alias_first = ArchiveSession::create(&alias, 4096).unwrap();
    let error = match ArchiveSession::create(&real, 4096) {
        Ok(_) => panic!("real archive unexpectedly acquired the alias authority"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("ArchiveAlreadyLocked"));
    drop(alias_first);

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn concurrent_creation_of_missing_archive_has_one_winner() {
    let role = std::env::var_os("MEMORIA_ARCHIVE_CREATE_RACE_ROLE");
    if let Some(role) = role {
        let root = PathBuf::from(std::env::var_os("MEMORIA_ARCHIVE_CREATE_RACE_ROOT").unwrap());
        let ready = root.join(format!("ready-{}", role.to_string_lossy()));
        let go = root.join("go");
        let outcome = root.join(format!("outcome-{}", role.to_string_lossy()));
        let release = root.join("release");
        fs::write(&ready, b"ready").unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        while !go.exists() {
            assert!(Instant::now() < deadline, "parent did not release the race");
            thread::sleep(Duration::from_millis(10));
        }

        match ArchiveSession::create(&root.join("archive-root"), 4096) {
            Ok(session) => {
                fs::write(&outcome, b"WON").unwrap();
                let deadline = Instant::now() + Duration::from_secs(10);
                while !release.exists() {
                    assert!(
                        Instant::now() < deadline,
                        "parent did not release the winner"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                drop(session);
                fs::write(root.join("winner-done"), b"done").unwrap();
            }
            Err(error) => {
                assert!(
                    error.to_string().contains("ArchiveAlreadyLocked"),
                    "loser returned an unexpected error: {error}"
                );
                fs::write(&outcome, b"LOCKED").unwrap();
            }
        }
        return;
    }

    let root = test_root("concurrent-create");
    let target = root.join("archive-root");
    assert!(!target.exists());
    let executable = std::env::current_exe().unwrap();
    let mut children = Vec::new();
    for role in ["a", "b"] {
        children.push(
            Command::new(&executable)
                .args([
                    "--exact",
                    "concurrent_creation_of_missing_archive_has_one_winner",
                    "--nocapture",
                ])
                .env("MEMORIA_ARCHIVE_CREATE_RACE_ROLE", role)
                .env("MEMORIA_ARCHIVE_CREATE_RACE_ROOT", &root)
                .spawn()
                .unwrap(),
        );
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    while !(root.join("ready-a").exists() && root.join("ready-b").exists()) {
        assert!(Instant::now() < deadline, "children did not become ready");
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!target.exists(), "a child created the archive before GO");
    fs::write(root.join("go"), b"go").unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while !(root.join("outcome-a").exists() && root.join("outcome-b").exists()) {
        assert!(
            Instant::now() < deadline,
            "children did not report outcomes"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let outcomes = [
        fs::read(root.join("outcome-a")).unwrap(),
        fs::read(root.join("outcome-b")).unwrap(),
    ];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| outcome.as_slice() == b"WON")
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| outcome.as_slice() == b"LOCKED")
            .count(),
        1
    );
    assert!(target.is_dir());

    let error = match ArchiveSession::create(&target, 4096) {
        Ok(_) => panic!("a third writer acquired the winner's authority"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("ArchiveAlreadyLocked"));

    fs::write(root.join("release"), b"release").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("winner-done").exists() {
        assert!(
            Instant::now() < deadline,
            "winner did not release the authority"
        );
        thread::sleep(Duration::from_millis(10));
    }
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }

    let reopened = ArchiveSession::create(&target, 4096).unwrap();
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn writer_lock_rendezvous_is_stable_across_archive_creation() {
    let role = std::env::var_os("MEMORIA_ARCHIVE_RENDEZVOUS_ROLE");
    if let Some(role) = role {
        let root = PathBuf::from(std::env::var_os("MEMORIA_ARCHIVE_RENDEZVOUS_ROOT").unwrap());
        let target = root.join("archive-root");
        let release = root.join("release");
        let outcome = root.join(format!("outcome-{}", role.to_string_lossy()));

        match role.to_string_lossy().as_ref() {
            "a" => {
                let session = ArchiveSession::create(&target, 4096).unwrap();
                assert!(target.exists());
                fs::write(root.join("created-and-holding"), b"CREATED_AND_HOLDING").unwrap();
                let deadline = Instant::now() + Duration::from_secs(10);
                while !release.exists() {
                    assert!(Instant::now() < deadline, "parent did not release writer A");
                    thread::sleep(Duration::from_millis(10));
                }
                drop(session);
                fs::write(&outcome, b"RELEASED").unwrap();
            }
            "b" => {
                let error = match ArchiveSession::create(&target, 4096) {
                    Ok(_) => panic!("writer B unexpectedly acquired the archive authority"),
                    Err(error) => error,
                };
                assert!(
                    error.to_string().contains("ArchiveAlreadyLocked"),
                    "writer B returned an unexpected error: {error}"
                );
                fs::write(&outcome, b"LOCKED").unwrap();
            }
            other => panic!("unexpected rendezvous test role: {other}"),
        }
        return;
    }

    let root = test_root("stable-rendezvous");
    let target = root.join("archive-root");
    assert!(!target.exists());
    let executable = std::env::current_exe().unwrap();
    let mut writer_a = Command::new(&executable)
        .args([
            "--exact",
            "writer_lock_rendezvous_is_stable_across_archive_creation",
            "--nocapture",
        ])
        .env("MEMORIA_ARCHIVE_RENDEZVOUS_ROLE", "a")
        .env("MEMORIA_ARCHIVE_RENDEZVOUS_ROOT", &root)
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("created-and-holding").exists() {
        assert!(
            Instant::now() < deadline,
            "writer A did not create the archive"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(target.exists());

    let mut writer_b = Command::new(&executable)
        .args([
            "--exact",
            "writer_lock_rendezvous_is_stable_across_archive_creation",
            "--nocapture",
        ])
        .env("MEMORIA_ARCHIVE_RENDEZVOUS_ROLE", "b")
        .env("MEMORIA_ARCHIVE_RENDEZVOUS_ROOT", &root)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("outcome-b").exists() {
        assert!(
            Instant::now() < deadline,
            "writer B did not report its result"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fs::read(root.join("outcome-b")).unwrap(), b"LOCKED");
    assert!(writer_b.wait().unwrap().success());

    let error = match ArchiveSession::create(&target, 4096) {
        Ok(_) => panic!("a third writer acquired writer A's authority"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("ArchiveAlreadyLocked"));

    fs::write(root.join("release"), b"RELEASE").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("outcome-a").exists() {
        assert!(
            Instant::now() < deadline,
            "writer A did not release its session"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fs::read(root.join("outcome-a")).unwrap(), b"RELEASED");
    assert!(writer_a.wait().unwrap().success());

    let reopened = ArchiveSession::create(&target, 4096).unwrap();
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reset_uses_stable_rendezvous_outside_deleted_tree() {
    let root = test_root("reset");
    let session = ArchiveSession::create(&root, 4096).unwrap();
    let error = match ArchiveSession::reset(&root, 4096) {
        Ok(_) => panic!("reset unexpectedly acquired an active archive"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("ArchiveAlreadyLocked"));
    assert!(root.join("archive").is_dir());
    drop(session);

    let reset = ArchiveSession::reset(&root, 4096).unwrap();
    drop(reset);
    assert!(root.join("archive").is_dir());
    let rendezvous = root.parent().unwrap().join(format!(
        ".memoria-{}-archive.writer.lock",
        root.file_name().unwrap().to_string_lossy()
    ));
    assert!(rendezvous.is_file());
    fs::remove_dir_all(&root).unwrap();
    fs::remove_file(rendezvous).unwrap();
}

#[test]
fn child_process_lock_is_released_after_kill() {
    if std::env::var_os("MEMORIA_ARCHIVE_LOCK_HOLDER").is_some() {
        let root = PathBuf::from(std::env::var_os("MEMORIA_ARCHIVE_LOCK_HOLDER").unwrap());
        let _writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        fs::write(root.join("ready"), b"ready").unwrap();
        thread::sleep(Duration::from_secs(60));
        return;
    }

    let root = test_root("process-lock");
    let mut child: Child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_process_lock_is_released_after_kill",
            "--nocapture",
        ])
        .env("MEMORIA_ARCHIVE_LOCK_HOLDER", &root)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("ready").exists() {
        assert!(
            Instant::now() < deadline,
            "child did not acquire archive lock"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let error = ArchiveWriter::open(&root.join("archive"), 4096).unwrap_err();
    assert!(error.to_string().contains("ArchiveAlreadyLocked"));
    child.kill().unwrap();
    child.wait().unwrap();
    let writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
    drop(writer);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn child_process_normal_exit_releases_lock() {
    if std::env::var_os("MEMORIA_ARCHIVE_LOCK_HOLDER").is_some() {
        let root = PathBuf::from(std::env::var_os("MEMORIA_ARCHIVE_LOCK_HOLDER").unwrap());
        let _writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        fs::write(root.join("ready"), b"ready").unwrap();
        return;
    }

    let root = test_root("process-normal-exit");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_process_normal_exit_releases_lock",
            "--nocapture",
        ])
        .env("MEMORIA_ARCHIVE_LOCK_HOLDER", &root)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("ready").exists() {
        assert!(
            Instant::now() < deadline,
            "child did not acquire archive lock"
        );
        thread::sleep(Duration::from_millis(20));
    }
    child.wait().unwrap();
    let writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
    drop(writer);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_reset_is_refused_while_another_process_holds_the_archive() {
    if std::env::var_os("MEMORIA_ARCHIVE_LOCK_HOLDER").is_some() {
        let root = PathBuf::from(std::env::var_os("MEMORIA_ARCHIVE_LOCK_HOLDER").unwrap());
        let _writer = ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        fs::write(root.join("ready"), b"ready").unwrap();
        thread::sleep(Duration::from_secs(60));
        return;
    }

    let root = test_root("cli-reset");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cli_reset_is_refused_while_another_process_holds_the_archive",
            "--nocapture",
        ])
        .env("MEMORIA_ARCHIVE_LOCK_HOLDER", &root)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("ready").exists() {
        assert!(
            Instant::now() < deadline,
            "child did not acquire archive lock"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let result = Command::new(env!("CARGO_BIN_EXE_mail-archive-experiment"))
        .args(["benchmark", "--out"])
        .arg(&root)
        .args(["--messages", "0"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(root.join("archive").is_dir());
    child.kill().unwrap();
    child.wait().unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_mail-archive-experiment"))
        .args(["benchmark", "--out"])
        .arg(&root)
        .args(["--messages", "0"])
        .output()
        .unwrap();
    assert!(result.status.success(), "CLI reset failed after release");
    fs::remove_dir_all(&root).unwrap();
    let rendezvous = root.parent().unwrap().join(format!(
        ".memoria-{}-archive.writer.lock",
        root.file_name().unwrap().to_string_lossy()
    ));
    fs::remove_file(rendezvous).unwrap();
}
