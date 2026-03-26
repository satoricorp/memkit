use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("memkit build requires CARGO_MANIFEST_DIR"),
    );
    let git_hash = git_output(&manifest_dir, &["rev-parse", "--short", "HEAD"]);
    let git_dir_raw = git_output(&manifest_dir, &["rev-parse", "--git-dir"]);
    let git_dir = resolve_git_dir(&manifest_dir, git_dir_raw.trim());

    println!("cargo:rustc-env=MEMKIT_BUILD_VERSION={}", git_hash);
    emit_git_rerun_hints(&git_dir);
}

fn git_output(manifest_dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| {
            panic!(
                "memkit build requires git metadata: failed to run `git {}`: {}",
                args.join(" "),
                err
            )
        });

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "memkit build requires git metadata: `git {}` failed: {}",
            args.join(" "),
            stderr.trim()
        );
    }

    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|err| panic!("memkit build requires utf-8 git output: {}", err));
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        panic!(
            "memkit build requires git metadata: `git {}` returned empty output",
            args.join(" ")
        );
    }
    trimmed.to_string()
}

fn resolve_git_dir(manifest_dir: &Path, git_dir_raw: &str) -> PathBuf {
    let git_dir = PathBuf::from(git_dir_raw);
    if git_dir.is_absolute() {
        git_dir
    } else {
        manifest_dir.join(git_dir)
    }
}

fn emit_git_rerun_hints(git_dir: &Path) {
    let head_path = git_dir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head_path.display());

    let head_contents = fs::read_to_string(&head_path).unwrap_or_else(|err| {
        panic!(
            "memkit build requires git metadata: failed to read {}: {}",
            head_path.display(),
            err
        )
    });
    if let Some(head_ref) = head_contents.trim().strip_prefix("ref: ") {
        println!(
            "cargo:rerun-if-changed={}",
            git_dir.join(head_ref).display()
        );
    }

    println!(
        "cargo:rerun-if-changed={}",
        git_dir.join("packed-refs").display()
    );
}
