use sem_core::model::change::{ChangeType, SemanticChange};
use sem_core::parser::differ::DiffResult;
use std::collections::BTreeMap;

pub fn format_nice(result: &DiffResult) -> String {
    let entity_changes: Vec<&SemanticChange> = result
        .changes
        .iter()
        .filter(|c| c.entity_type != "orphan")
        .collect();

    if entity_changes.is_empty() {
        return "No semantic changes detected.\n".to_string();
    }

    let mut out = String::new();

    let mut by_file: BTreeMap<&str, Vec<&SemanticChange>> = BTreeMap::new();
    for c in &entity_changes {
        by_file.entry(&c.file_path).or_default().push(c);
    }

    for (file_path, changes) in &by_file {
        let n = changes.len();
        out.push_str(&format!(
            "{} — {} change{}\n\n",
            file_path,
            n,
            if n == 1 { "" } else { "s" }
        ));

        let order = [
            (ChangeType::Modified, "MODIFIED"),
            (ChangeType::Added, "ADDED"),
            (ChangeType::Deleted, "DELETED"),
            (ChangeType::Renamed, "RENAMED"),
            (ChangeType::Moved, "MOVED"),
            (ChangeType::Reordered, "REORDERED"),
        ];

        for (kind, label) in order {
            let group: Vec<&&SemanticChange> =
                changes.iter().filter(|c| c.change_type == kind).collect();
            if group.is_empty() {
                continue;
            }
            out.push_str(&format!("{} ({})\n", label, group.len()));
            for c in group {
                out.push_str(&format_change(c));
            }
            out.push('\n');
        }
    }

    let mut parts: Vec<String> = Vec::new();
    if result.modified_count > 0 {
        parts.push(format!("{} modified", result.modified_count));
    }
    if result.added_count > 0 {
        parts.push(format!("{} added", result.added_count));
    }
    if result.deleted_count > 0 {
        parts.push(format!("{} deleted", result.deleted_count));
    }
    if result.renamed_count > 0 {
        parts.push(format!("{} renamed", result.renamed_count));
    }
    if result.moved_count > 0 {
        parts.push(format!("{} moved", result.moved_count));
    }
    if result.reordered_count > 0 {
        parts.push(format!("{} reordered", result.reordered_count));
    }

    out.push_str(&format!("Summary: {}\n", parts.join(", ")));
    out
}

fn full_path(c: &SemanticChange) -> String {
    match &c.parent_name {
        Some(p) => format!("{}::{}", p, c.entity_name),
        None => c.entity_name.clone(),
    }
}

/// Render an entity ID as a `::`-joined human path. For JSON IDs like
/// `data.json::/action/createPayment`, returns `action::createPayment`.
/// For other shapes, falls back to the last `::` segment.
fn entity_id_to_path(entity_id: &str, file_path: &str) -> String {
    let prefix = format!("{}::", file_path);
    if let Some(rest) = entity_id.strip_prefix(&prefix) {
        if let Some(pointer) = rest.strip_prefix('/') {
            return pointer.replace('/', "::");
        }
    }
    entity_id
        .rsplit("::")
        .next()
        .unwrap_or(entity_id)
        .to_string()
}

fn format_change(c: &SemanticChange) -> String {
    let path = full_path(c);

    match c.change_type {
        ChangeType::Modified => {
            let mut s = format!("  {}\n", path);
            if c.structural_change == Some(false) {
                s.push_str("    (formatting only)\n");
                return s;
            }
            if let Some(before) = &c.before_content {
                let trimmed = before.trim();
                if !trimmed.is_empty() {
                    s.push_str(&format!("    - {}\n", trimmed));
                }
            }
            if let Some(after) = &c.after_content {
                let trimmed = after.trim();
                if !trimmed.is_empty() {
                    s.push_str(&format!("    + {}\n", trimmed));
                }
            }
            s
        }
        ChangeType::Added => {
            let mut s = format!("  {}\n", path);
            if let Some(after) = &c.after_content {
                let trimmed = after.trim();
                if !trimmed.is_empty() {
                    s.push_str(&format!("    + {}\n", trimmed));
                }
            }
            s
        }
        ChangeType::Deleted => {
            let mut s = format!("  {}\n", path);
            if let Some(before) = &c.before_content {
                let trimmed = before.trim();
                if !trimmed.is_empty() {
                    s.push_str(&format!("    - {}\n", trimmed));
                }
            }
            s
        }
        ChangeType::Renamed => {
            let old_path = match (&c.parent_name, &c.old_entity_name) {
                (Some(p), Some(old)) => format!("{}::{}", p, old),
                (None, Some(old)) => old.clone(),
                _ => path.clone(),
            };
            format!("  {} → {}\n", old_path, path)
        }
        ChangeType::Moved => {
            let old_name = c.old_entity_name.as_deref().unwrap_or(&c.entity_name);
            let old_path_str = if let Some(old_pid) = &c.old_parent_id {
                let chain = entity_id_to_path(old_pid, &c.file_path);
                format!("{}::{}", chain, old_name)
            } else if let Some(old_file) = &c.old_file_path {
                format!("{}:{}", old_file, old_name)
            } else {
                old_name.to_string()
            };
            format!("  {} → {}\n", old_path_str, path)
        }
        ChangeType::Reordered => format!("  {}\n", path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sem_core::git::types::{FileChange, FileStatus};
    use sem_core::parser::differ::compute_semantic_diff;
    use sem_core::parser::plugins::json::JsonParserPlugin;
    use sem_core::parser::registry::ParserRegistry;

    fn run_diff(before: &str, after: &str) -> DiffResult {
        let mut registry = ParserRegistry::new();
        registry.register(Box::new(JsonParserPlugin));
        compute_semantic_diff(
            &[FileChange {
                file_path: "data.json".to_string(),
                status: FileStatus::Modified,
                old_file_path: None,
                before_content: Some(before.to_string()),
                after_content: Some(after.to_string()),
            }],
            &registry,
            None,
            None,
        )
    }

    #[test]
    fn empty_diff_says_no_changes() {
        let result = DiffResult {
            changes: vec![],
            file_count: 0,
            added_count: 0,
            modified_count: 0,
            deleted_count: 0,
            moved_count: 0,
            renamed_count: 0,
            reordered_count: 0,
            orphan_count: 0,
            total_entities_before: 0,
            total_entities_after: 0,
        };
        assert_eq!(format_nice(&result), "No semantic changes detected.\n");
    }

    #[test]
    fn nested_modified_uses_parent_chain_in_path() {
        let result = run_diff(
            "{\n  \"scripts\": {\n    \"build\": \"tsc\"\n  }\n}",
            "{\n  \"scripts\": {\n    \"build\": \"webpack\"\n  }\n}",
        );
        let out = format_nice(&result);
        assert!(out.contains("MODIFIED (1)"), "got:\n{out}");
        assert!(out.contains("scripts::build"), "got:\n{out}");
        assert!(out.contains("\"tsc\""), "got:\n{out}");
        assert!(out.contains("\"webpack\""), "got:\n{out}");
    }

    #[test]
    fn renamed_shows_old_arrow_new_with_parent() {
        let result = run_diff(
            "{\n  \"scripts\": {\n    \"run\": \"node .\"\n  }\n}",
            "{\n  \"scripts\": {\n    \"start\": \"node .\"\n  }\n}",
        );
        let out = format_nice(&result);
        assert!(out.contains("RENAMED (1)"), "got:\n{out}");
        assert!(out.contains("scripts::run → scripts::start"), "got:\n{out}");
    }

    #[test]
    fn moved_with_renamed_key_shows_both_paths_via_old_parent_id() {
        let result = run_diff(
            "{\n  \"action\": {\n    \"createPayment\": {\n      \"username\": \"x\",\n      \"password\": \"y\"\n    }\n  }\n}",
            "{\n  \"action\": {\n    \"IssueOperation\": {\n      \"id\": \"x\",\n      \"password\": \"y\"\n    }\n  }\n}",
        );
        let out = format_nice(&result);
        assert!(out.contains("MOVED"), "got:\n{out}");
        assert!(
            out.contains("action::createPayment::username → action::IssueOperation::id"),
            "got:\n{out}"
        );
        assert!(
            out.contains("action::createPayment::password → action::IssueOperation::password"),
            "got:\n{out}"
        );
    }

    #[test]
    fn cosmetic_modification_is_marked_as_formatting_only() {
        // Adding a sibling after `x` causes `x`'s content text to gain a
        // trailing comma — content_hash differs but structural_hash matches,
        // so the differ flags it as a non-structural (cosmetic) change.
        let result = run_diff("{\n  \"x\": 1\n}", "{\n  \"x\": 1,\n  \"y\": 2\n}");
        let out = format_nice(&result);
        let x_block_start = out.find("  x\n").expect("x change should be present");
        let x_block = &out[x_block_start..];
        assert!(x_block.contains("(formatting only)"), "got:\n{out}");
    }
}
