use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn expected_git_sha() -> String {
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

fn expected_release_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

fn expected_display_version() -> String {
    format!("{} ({})", expected_release_version(), expected_git_sha())
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

fn prepend_path(dir: &Path) -> String {
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![dir.to_path_buf()];
    paths.extend(std::env::split_paths(&existing));
    std::env::join_paths(paths)
        .expect("join PATH")
        .to_string_lossy()
        .to_string()
}

#[test]
fn version_commands_use_semver_and_git_short_sha() {
    let expected = format!("memkit {}", expected_display_version());

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
fn help_banner_uses_semver_and_git_short_sha() {
    let expected = format!("Version {}", expected_display_version());
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
fn build_succeeds_without_git_metadata_and_uses_semver() {
    let project_dir = unique_temp_dir("memkit-build-no-git");
    let src_dir = project_dir.join("src");
    let bin_dir = project_dir.join("bin");
    fs::create_dir_all(&src_dir).expect("create src dir");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    fs::write(
        project_dir.join("Cargo.toml"),
        "[package]\nname = \"memkit-build-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("write Cargo.toml");
    fs::write(project_dir.join("build.rs"), include_str!("../build.rs")).expect("write build.rs");
    fs::write(
        src_dir.join("main.rs"),
        r#"const SEMVER: &str = env!("MEMKIT_SEMVER");
const GIT_SHA: Option<&str> = option_env!("MEMKIT_GIT_SHA");

fn main() {
    match GIT_SHA {
        Some(sha) => println!("{SEMVER} ({sha})"),
        None => println!("{SEMVER}"),
    }
}
"#,
    )
    .expect("write main.rs");
    let git_shim = bin_dir.join("git");
    fs::write(&git_shim, "#!/bin/sh\nexit 1\n").expect("write git shim");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&git_shim)
            .expect("git shim metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&git_shim, perms).expect("chmod git shim");
    }

    let output = Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&project_dir)
        .env("CARGO_TARGET_DIR", project_dir.join("target"))
        .env("PATH", prepend_path(&bin_dir))
        .env_remove("MEMKIT_GIT_SHA")
        .env_remove("MEMKIT_SEMVER")
        .env("RUST_BACKTRACE", "0")
        .output()
        .expect("run cargo build");

    assert!(
        output.status.success(),
        "cargo build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let built_binary = built_binary_path(&project_dir.join("target"), "memkit-build-fixture");
    let output = Command::new(&built_binary)
        .output()
        .expect("run built fixture binary");
    assert!(
        output.status.success(),
        "fixture binary failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "0.1.0\n");

    let _ = fs::remove_dir_all(project_dir);
}

fn built_binary_path(target_dir: &Path, binary_name: &str) -> PathBuf {
    let mut path = target_dir.join("debug").join(binary_name);
    if cfg!(windows) {
        path.set_extension("exe");
    }
    path
}
