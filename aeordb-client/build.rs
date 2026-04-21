use std::path::Path;

fn main() {
  // Copy shared web components into static/shared/ before rust_embed processes them.
  let shared_src = Path::new("../../../aeordb-web-components");
  let shared_dst = Path::new("static/shared");

  // Use the workspace-relative path if the above doesn't exist (CI, etc.)
  let src = if shared_src.exists() {
    shared_src
  } else {
    // Fallback: try relative to workspace root
    let alt = Path::new("../../aeordb-web-components");
    if alt.exists() { alt } else { shared_src }
  };

  if src.exists() {
    // Remove old copy
    if shared_dst.exists() {
      let _ = std::fs::remove_dir_all(shared_dst);
    }

    copy_dir_recursive(src, shared_dst)
      .expect("failed to copy shared web components");

    // Tell cargo to re-run if the source changes
    println!("cargo:rerun-if-changed={}", src.display());
  } else {
    println!("cargo:warning=aeordb-web-components not found at {}, skipping shared copy", src.display());
  }

  tauri_build::build();
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
  std::fs::create_dir_all(dst)?;

  for entry in std::fs::read_dir(src)? {
    let entry = entry?;
    let file_type = entry.file_type()?;
    let src_path = entry.path();
    let dst_path = dst.join(entry.file_name());

    // Skip hidden files and .git
    let name = entry.file_name();
    let name_str = name.to_string_lossy();
    if name_str.starts_with('.') {
      continue;
    }

    if file_type.is_dir() {
      copy_dir_recursive(&src_path, &dst_path)?;
    } else {
      std::fs::copy(&src_path, &dst_path)?;
    }
  }

  Ok(())
}
