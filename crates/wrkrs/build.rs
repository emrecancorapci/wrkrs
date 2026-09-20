//! Build script: embeds the version from git describe, the way wrk
//! compiles version.o.

fn main() {
    let described = std::process::Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output();
    let version = match described {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
        // Tarball builds fall back to the crate version.
        _ => "0.1.0".to_owned(),
    };
    println!("cargo:rustc-env=WRKRS_VERSION={version}");

    if std::path::Path::new(".git/HEAD").exists() {
        println!("cargo:rerun-if-changed=.git/HEAD");
    }
}
