use std::path::Path;
use std::process::Command;

fn main() {
  // Rsync aeordb-web-components (DB-specific) into static/shared/
  rsync_repo(
    &["../../../aeordb-web-components", "../../aeordb-web-components"],
    "static/shared/",
    "aeordb-web-components",
  );

  // Rsync aeor-web-components (generic) into static/aeor/
  rsync_repo(
    &["../../../aeor-web-components", "../../../../aeor-web-components"],
    "static/aeor/",
    "aeor-web-components",
  );

  tauri_build::build();
}

fn rsync_repo(candidates: &[&str], dst: &str, label: &str) {
  let src = candidates.iter()
    .map(|p| Path::new(p))
    .find(|p| p.exists());

  let Some(src) = src else {
    println!("cargo:warning={} not found, skipping sync", label);
    return;
  };

  let src_str = format!("{}/", src.display()); // trailing slash = copy contents

  let status = Command::new("rsync")
    .args([
      "-a",            // archive mode (recursive, preserves timestamps/permissions)
      "--delete",      // remove files in dst that don't exist in src
      "--exclude", ".*", // skip hidden files (.git, etc.)
      &src_str,
      dst,
    ])
    .status()
    .expect("failed to run rsync — is it installed?");

  if !status.success() {
    panic!("rsync of {} failed with exit code: {:?}", label, status.code());
  }

  println!("cargo:rerun-if-changed={}", src.display());
}
