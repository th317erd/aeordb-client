fn main() {
  // rust-embed embeds files from static/ at compile time and follows symlinks
  // (static/aeor and static/shared point at sibling repos). Cargo's normal
  // include_bytes! tracking handles per-file changes, but ADDED or REMOVED
  // files in the source repos won't retrigger the proc-macro without these.
  println!("cargo:rerun-if-changed=static/aeor");
  println!("cargo:rerun-if-changed=static/shared");

  tauri_build::build();
}
