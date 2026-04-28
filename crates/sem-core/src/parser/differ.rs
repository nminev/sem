use rayon::prelude::*;
use serde::Serialize;

use crate::git::types::FileChange;
use crate::model::change::{ChangeType, SemanticChange};
use crate::model::identity::match_entities;
use crate::parser::registry::ParserRegistry;
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffResult {
    pub changes: Vec<SemanticChange>,
    pub file_count: usize,
    pub added_count: usize,
    pub modified_count: usize,
    pub deleted_count: usize,
    pub moved_count: usize,
    pub renamed_count: usize,
}

pub fn compute_semantic_diff(
    file_changes: &[FileChange],
    registry: &ParserRegistry,
    commit_sha: Option<&str>,
    author: Option<&str>,
) -> DiffResult {
    // Process files in parallel: each file's entity extraction and matching is independent
    let per_file_changes: Vec<(String, Vec<SemanticChange>)> = file_changes
        .par_iter()
        .filter_map(|file| {
            let content_hint = file.after_content.as_deref()
                .or(file.before_content.as_deref())
                .unwrap_or("");
            let plugin = registry.get_plugin_with_content(&file.file_path, content_hint)?;

            let before_entities = if let Some(ref content) = file.before_content {
                let before_path = file.old_file_path.as_deref().unwrap_or(&file.file_path);
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    plugin.extract_entities(content, before_path)
                })) {
                    Ok(entities) => entities,
                    Err(_) => Vec::new(),
                }
            } else {
                Vec::new()
            };

            let after_entities = if let Some(ref content) = file.after_content {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    plugin.extract_entities(content, &file.file_path)
                })) {
                    Ok(entities) => entities,
                    Err(_) => Vec::new(),
                }
            } else {
                Vec::new()
            };

            let sim_fn = |a: &crate::model::entity::SemanticEntity,
                          b: &crate::model::entity::SemanticEntity|
             -> f64 { plugin.compute_similarity(a, b) };

            let mut result = match_entities(
                &before_entities,
                &after_entities,
                &file.file_path,
                Some(&sim_fn),
                commit_sha,
                author,
            );

            result.changes.sort_by_key(|change| change.entity_line);

            if result.changes.is_empty() {
                None
            } else {
                Some((file.file_path.clone(), result.changes))
            }
        })
        .collect();

    let mut all_changes: Vec<SemanticChange> = Vec::new();
    let mut files_with_changes: HashSet<String> = HashSet::new();
    for (file_path, changes) in per_file_changes {
        files_with_changes.insert(file_path);
        all_changes.extend(changes);
    }

    // Single-pass counting
    let mut added_count = 0;
    let mut modified_count = 0;
    let mut deleted_count = 0;
    let mut moved_count = 0;
    let mut renamed_count = 0;

    for c in &all_changes {
        match c.change_type {
            ChangeType::Added => added_count += 1,
            ChangeType::Modified => modified_count += 1,
            ChangeType::Deleted => deleted_count += 1,
            ChangeType::Moved => moved_count += 1,
            ChangeType::Renamed => renamed_count += 1,
        }
    }

    DiffResult {
        changes: all_changes,
        file_count: files_with_changes.len(),
        added_count,
        modified_count,
        deleted_count,
        moved_count,
        renamed_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::types::{FileChange, FileStatus};
    use crate::parser::plugins::create_default_registry;

    fn file_change(path: &str, before: &str, after: &str) -> FileChange {
        FileChange {
            file_path: path.to_string(),
            status: FileStatus::Modified,
            old_file_path: None,
            before_content: Some(before.to_string()),
            after_content: Some(after.to_string()),
        }
    }

    /// When only a method body changes and the class declaration is unchanged,
    /// the class should be suppressed — only the method shows as Modified.
    #[test]
    fn test_parent_suppressed_when_only_child_modified() {
        let before = "class UserService:\n    def get_user(self, user_id):\n        return db.find(user_id)\n";
        let after  = "class UserService:\n    def get_user(self, user_id):\n        return db.find(user_id, include_deleted=False)\n";

        let registry = create_default_registry();
        let result = compute_semantic_diff(&[file_change("svc.py", before, after)], &registry, None, None);

        let names: Vec<&str> = result.changes.iter().map(|c| c.entity_name.as_str()).collect();
        assert!(
            result.changes.iter().any(|c| c.entity_name == "get_user" && c.change_type == ChangeType::Modified),
            "expected method get_user to be Modified, got: {names:?}"
        );
        assert!(
            !result.changes.iter().any(|c| c.entity_name == "UserService" && c.change_type == ChangeType::Modified),
            "class UserService should be suppressed when only the method body changed, got: {names:?}"
        );
    }

    /// When the class declaration itself changes (gains a base class) *and* a child
    /// method also changes, the parent Modified should NOT be suppressed — both
    /// changes are meaningful and should appear in the output.
    #[test]
    fn test_parent_not_suppressed_when_own_declaration_changes() {
        let before = "class UserService:\n    def get_user(self, user_id):\n        return db.find(user_id)\n";
        let after  = "class UserService(BaseService):\n    def get_user(self, user_id):\n        return db.find(user_id, include_deleted=False)\n";

        let registry = create_default_registry();
        let result = compute_semantic_diff(&[file_change("svc.py", before, after)], &registry, None, None);

        let names: Vec<&str> = result.changes.iter().map(|c| c.entity_name.as_str()).collect();
        assert!(
            result.changes.iter().any(|c| c.entity_name == "get_user" && c.change_type == ChangeType::Modified),
            "expected method get_user to be Modified, got: {names:?}"
        );
        assert!(
            result.changes.iter().any(|c| c.entity_name == "UserService" && c.change_type == ChangeType::Modified),
            "class UserService should remain Modified when its own declaration changed, got: {names:?}"
        );
    }
}
