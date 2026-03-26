use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn expected_git_version() -> String {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("git rev-parse");
    assert!(
        output.status.success(),
        "git rev-parse failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git rev-parse utf8")
        .trim()
        .to_string()
}

fn run_mk(args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mk"));
    command.args(args);
    command.env("NO_COLOR", "1");
    command.output().expect("run mk")
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{}-{}-{}", prefix, std::process::id(), ts));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn version_commands_use_git_short_sha() {
    let expected = format!("memkit {}", expected_git_version());

    for args in [&["version"][..], &["-V"][..]] {
        let output = run_mk(args);
        assert!(
            output.status.success(),
            "mk {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("{}\n", expected)
        );
    }
}

#[test]
fn help_banner_uses_git_short_sha() {
    let expected = format!("Version {}", expected_git_version());
    let output = run_mk(&["help"]);
    assert!(
        output.status.success(),
        "mk help failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("mk help stdout utf8");
    assert!(
        stdout.contains(&expected),
        "help output missing build version `{}`\nstdout:\n{}",
        expected,
        stdout
    );
}

#[test]
fn build_fails_without_git_metadata() {
    let project_dir = unique_temp_dir("memkit-build-no-git");
    let src_dir = project_dir.join("src");
    fs::create_dir_all(&src_dir).expect("create src dir");
    fs::write(
        project_dir.join("Cargo.toml"),
        "[package]\nname = \"memkit-build-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write Cargo.toml");
    fs::write(project_dir.join("build.rs"), include_str!("../build.rs")).expect("write build.rs");
    fs::write(src_dir.join("main.rs"), "fn main() {}\n").expect("write main.rs");

    let output = Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&project_dir)
        .env("CARGO_TARGET_DIR", project_dir.join("target"))
        .env("RUST_BACKTRACE", "0")
        .output()
        .expect("run cargo build");

    assert!(
        !output.status.success(),
        "cargo build unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8(output.stderr).expect("cargo build stderr utf8");
    assert!(
        stderr.contains("memkit build requires git metadata"),
        "build failure did not mention git metadata\nstderr:\n{}",
        stderr
    );

    let _ = fs::remove_dir_all(project_dir);
}
