/// Parse a filter string into individual glob patterns.
/// Supports comma-separated patterns and `!` prefix for exclusion.
///
/// Examples:
///   "*.pdf"           → include only PDFs
///   "*.pdf, *.md"     → include PDFs and markdown
///   "!*.tmp, !.DS_Store" → exclude tmp files and .DS_Store
///   "*.pdf, !draft-*" → include PDFs but exclude drafts
pub fn parse_filter(filter: &str) -> Vec<FilterPattern> {
  filter
    .split(',')
    .map(|pattern| pattern.trim())
    .filter(|pattern| !pattern.is_empty())
    .map(|pattern| {
      if let Some(stripped) = pattern.strip_prefix('!') {
        FilterPattern {
          pattern: stripped.trim().to_string(),
          exclude: true,
        }
      } else {
        FilterPattern {
          pattern: pattern.to_string(),
          exclude: false,
        }
      }
    })
    .collect()
}

#[derive(Debug, Clone)]
pub struct FilterPattern {
  pub pattern: String,
  pub exclude: bool,
}

/// Check if a filename matches the filter configuration.
/// If no filter is set, everything passes.
///
/// Logic:
/// 1. If there are include patterns, the file must match at least one.
/// 2. If the file matches any exclude pattern, it's rejected.
/// 3. If there are only exclude patterns, everything else passes.
pub fn matches_filter(filename: &str, filter: Option<&str>) -> bool {
  let filter_str = match filter {
    Some(f) if !f.is_empty() => f,
    _ => return true,
  };

  let patterns       = parse_filter(filter_str);
  let includes: Vec<_> = patterns.iter().filter(|p| !p.exclude).collect();
  let excludes: Vec<_> = patterns.iter().filter(|p| p.exclude).collect();

  // Check excludes first — if any exclude matches, reject
  for exclude in &excludes {
    if glob_match::glob_match(&exclude.pattern, filename) {
      return false;
    }
  }

  // If there are include patterns, file must match at least one
  if !includes.is_empty() {
    return includes.iter().any(|include| {
      glob_match::glob_match(&include.pattern, filename)
    });
  }

  // Only exclude patterns exist, and none matched — file passes
  true
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_no_filter_passes_everything() {
    assert!(matches_filter("anything.txt", None));
    assert!(matches_filter("anything.txt", Some("")));
  }

  #[test]
  fn test_single_include_pattern() {
    assert!(matches_filter("report.pdf", Some("*.pdf")));
    assert!(!matches_filter("report.txt", Some("*.pdf")));
  }

  #[test]
  fn test_multiple_include_patterns() {
    let filter = "*.pdf, *.md";
    assert!(matches_filter("report.pdf", Some(filter)));
    assert!(matches_filter("readme.md", Some(filter)));
    assert!(!matches_filter("image.png", Some(filter)));
  }

  #[test]
  fn test_exclude_pattern() {
    let filter = "!*.tmp";
    assert!(matches_filter("report.pdf", Some(filter)));
    assert!(!matches_filter("cache.tmp", Some(filter)));
  }

  #[test]
  fn test_mixed_include_and_exclude() {
    let filter = "*.pdf, !draft-*";
    assert!(matches_filter("report.pdf", Some(filter)));
    assert!(!matches_filter("draft-report.pdf", Some(filter)));
    assert!(!matches_filter("image.png", Some(filter)));
  }

  #[test]
  fn test_exclude_dotfiles() {
    let filter = "!.DS_Store, !.gitignore";
    assert!(matches_filter("readme.md", Some(filter)));
    assert!(!matches_filter(".DS_Store", Some(filter)));
    assert!(!matches_filter(".gitignore", Some(filter)));
  }

  #[test]
  fn test_wildcard_patterns() {
    assert!(matches_filter("report-2024.pdf", Some("report-*.pdf")));
    assert!(!matches_filter("summary-2024.pdf", Some("report-*.pdf")));
  }
}
