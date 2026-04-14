use crate::sync::relationships::SyncRelationship;

/// Given a list of all sync relationships, determine which remote sub-paths
/// are owned by child relationships and should be excluded from the parent.
///
/// Returns the list of remote path prefixes that the given relationship
/// should skip because a more specific relationship owns them.
pub fn child_exclusions(
  relationship: &SyncRelationship,
  all_relationships: &[SyncRelationship],
) -> Vec<String> {
  let mut exclusions = Vec::new();

  for other in all_relationships {
    // Skip self
    if other.id == relationship.id {
      continue;
    }

    // Skip if not on the same connection
    if other.remote_connection_id != relationship.remote_connection_id {
      continue;
    }

    // Check if the other relationship's remote path is a child of ours
    if other.remote_path.starts_with(&relationship.remote_path)
      && other.remote_path != relationship.remote_path
    {
      exclusions.push(other.remote_path.clone());
    }
  }

  exclusions
}

/// Check if a remote path should be excluded because it falls within
/// a child relationship's territory.
pub fn is_excluded_by_child(remote_path: &str, exclusions: &[String]) -> bool {
  exclusions.iter().any(|exclusion| remote_path.starts_with(exclusion))
}

#[cfg(test)]
mod tests {
  use super::*;
  use chrono::Utc;
  use crate::sync::relationships::{DeletePropagation, SyncDirection};

  fn make_relationship(id: &str, connection_id: &str, remote_path: &str) -> SyncRelationship {
    SyncRelationship {
      id:                   id.to_string(),
      name:                 id.to_string(),
      remote_connection_id: connection_id.to_string(),
      remote_path:          remote_path.to_string(),
      local_path:           format!("/tmp/{}", id),
      direction:            SyncDirection::Bidirectional,
      filter:               None,
      delete_propagation:   DeletePropagation::default(),
      enabled:              true,
      created_at:           Utc::now(),
      updated_at:           Utc::now(),
    }
  }

  #[test]
  fn test_child_exclusions_basic() {
    let parent = make_relationship("parent", "conn-1", "/projects/");
    let child  = make_relationship("child", "conn-1", "/projects/secrets/");

    let all = vec![parent.clone(), child.clone()];

    let exclusions = child_exclusions(&parent, &all);
    assert_eq!(exclusions, vec!["/projects/secrets/"]);

    // Child should have no exclusions
    let child_exclusions_result = child_exclusions(&child, &all);
    assert!(child_exclusions_result.is_empty());
  }

  #[test]
  fn test_child_exclusions_different_connections() {
    let parent = make_relationship("parent", "conn-1", "/projects/");
    let other  = make_relationship("other", "conn-2", "/projects/secrets/");

    let all = vec![parent.clone(), other.clone()];

    // Different connections — no exclusion
    let exclusions = child_exclusions(&parent, &all);
    assert!(exclusions.is_empty());
  }

  #[test]
  fn test_child_exclusions_multiple_children() {
    let parent = make_relationship("parent", "conn-1", "/data/");
    let child1 = make_relationship("child1", "conn-1", "/data/private/");
    let child2 = make_relationship("child2", "conn-1", "/data/archive/");

    let all = vec![parent.clone(), child1, child2];

    let exclusions = child_exclusions(&parent, &all);
    assert_eq!(exclusions.len(), 2);
    assert!(exclusions.contains(&"/data/private/".to_string()));
    assert!(exclusions.contains(&"/data/archive/".to_string()));
  }

  #[test]
  fn test_is_excluded_by_child() {
    let exclusions = vec!["/projects/secrets/".to_string()];

    assert!(is_excluded_by_child("/projects/secrets/key.json", &exclusions));
    assert!(is_excluded_by_child("/projects/secrets/deep/nested.txt", &exclusions));
    assert!(!is_excluded_by_child("/projects/readme.md", &exclusions));
    assert!(!is_excluded_by_child("/projects/src/main.rs", &exclusions));
  }

  #[test]
  fn test_same_path_not_excluded() {
    let parent = make_relationship("a", "conn-1", "/docs/");
    let same   = make_relationship("b", "conn-1", "/docs/");

    let all = vec![parent.clone(), same];

    // Same path — not a child, no exclusion
    let exclusions = child_exclusions(&parent, &all);
    assert!(exclusions.is_empty());
  }
}
