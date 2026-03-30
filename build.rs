use std::process::Command;

fn main() {
    // Re-run if git HEAD changes (new commit, checkout, tag)
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");

    let version = git_version().unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=SCRIBE_VERSION={version}");
}

fn git_version() -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--long"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let describe = String::from_utf8(output.stdout).ok()?.trim().to_string();

    // git describe --tags --long output: <tag>-<count>-g<hash>
    // e.g. "1.0.0-beta3-0-gabcdef1" or "1.0.0-5-gabcdef1"
    // We need to split from the right since tags can contain hyphens.
    let hash_sep = describe.rfind('-')?;
    let count_sep = describe[..hash_sep].rfind('-')?;

    let tag = &describe[..count_sep];
    let count = &describe[count_sep + 1..hash_sep];
    let hash = &describe[hash_sep + 1..]; // "gabcdef1"

    if count == "0" {
        // Exactly on a tagged commit
        Some(tag.to_string())
    } else {
        // Ahead of tag: <tag>-<count>-<hash>
        Some(format!("{tag}-{count}-{hash}"))
    }
}
