use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
    for profile in ["cargo", "git", "ripgrep", "threadmoth"] {
        println!("cargo:rerun-if-changed=../../profiles/{profile}.toml");
    }

    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("failed to run git rev-parse HEAD while building omen-cli");
    assert!(
        output.status.success(),
        "git rev-parse HEAD failed while building omen-cli"
    );

    let git_sha = String::from_utf8(output.stdout)
        .expect("git rev-parse HEAD returned non-UTF-8 output")
        .trim()
        .to_owned();
    assert!(
        !git_sha.is_empty(),
        "git rev-parse HEAD returned an empty SHA"
    );

    let target = std::env::var("TARGET").expect("TARGET is required by Cargo");
    let profile = std::env::var("PROFILE").expect("PROFILE is required by Cargo");
    let identity = format!(
        "{} contract:0.8 commit:{git_sha} target:{target} profile:{profile}",
        std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION is required by Cargo")
    );

    println!("cargo:rustc-env=OMEN_BUILD_IDENTITY={identity}");
    println!("cargo:rustc-env=OMEN_GIT_SHA={git_sha}");
}
