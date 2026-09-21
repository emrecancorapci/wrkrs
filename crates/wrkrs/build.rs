//! Build script: embeds the version from git describe, the way wrk
//! compiles version.o.

fn main() {
    use std::env;

    let described = std::process::Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output();
    let version = match described {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
        // Tarball builds fall back to the package version.
        _ => env::var("CARGO_PKG_VERSION").unwrap_or_default(),
    };
    println!("cargo:rustc-env=WRKRS_VERSION={version}");

    if std::path::Path::new(".git/HEAD").exists() {
        println!("cargo:rerun-if-changed=.git/HEAD");
    }
}
