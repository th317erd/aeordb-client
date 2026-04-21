use std::path::Path;
use std::process::Command;

fn main() {
  // Rsync shared web components into static/shared/ before rust_embed processes them.
  let shared_src = Path::new("../../../aeordb-web-components");
  let shared_dst = Path::new("static/shared/");

  let src = if shared_src.exists() {
    shared_src
  } else {
    let alt = Path::new("../../aeordb-web-components");
    if alt.exists() { alt } else { shared_src }
  };

  if src.exists() {
    let src_str = format!("{}/", src.display()); // trailing slash = copy contents

    let status = Command::new("rsync")
      .args([
        "-a",            // archive mode (recursive, preserves timestamps/permissions)
        "--delete",      // remove files in dst that don't exist in src
        "--exclude", ".*", // skip hidden files (.git, etc.)
        &src_str,
        shared_dst.to_str().unwrap(),
      ])
      .status()
      .expect("failed to run rsync — is it installed?");

    if !status.success() {
      panic!("rsync failed with exit code: {:?}", status.code());
    }

    println!("cargo:rerun-if-changed={}", src.display());
  } else {
    println!("cargo:warning=aeordb-web-components not found at {}, skipping shared sync", src.display());
  }

  tauri_build::build();
}
