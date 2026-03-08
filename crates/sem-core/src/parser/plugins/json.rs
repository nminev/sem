use crate::model::entity::{build_entity_id, SemanticEntity};
use crate::parser::plugin::SemanticParserPlugin;
use crate::utils::hash::content_hash;

pub struct JsonParserPlugin;

impl SemanticParserPlugin for JsonParserPlugin {
    fn id(&self) -> &str {
        "json"
    }

    fn extensions(&self) -> &[&str] {
        &[".json"]
    }

    fn extract_entities(&self, content: &str, file_path: &str) -> Vec<SemanticEntity> {
        let trimmed = content.trim();
        if !trimmed.starts_with('{') {
            return Vec::new();
        }

        let mut entities = Vec::new();
        extract_entries_recursive(content, file_path, 1, None, None, &mut entities);
        entities
    }
}

/// Recursively extract entities from a JSON object string.
///
/// - `content`: the full text of the object (including surrounding `{` `}`)
/// - `file_path`: original file path, threaded through for entity IDs
/// - `line_offset`: 1-based absolute line number of the first line of `content`
/// - `parent_pointer`: JSON Pointer prefix for children, e.g. `Some("/scripts")`
/// - `parent_entity_id`: the entity id of the enclosing entity (for `parent_id` field)
/// - `out`: collected entities, appended in-place (DFS pre-order)
fn extract_entries_recursive(
    content: &str,
    file_path: &str,
    line_offset: usize,
    parent_pointer: Option<&str>,
    parent_entity_id: Option<&str>,
    out: &mut Vec<SemanticEntity>,
) {
    let lines: Vec<&str> = content.lines().collect();
    let entries = find_top_level_entries(content);

    for (i, entry) in entries.iter().enumerate() {
        let end_line = if i + 1 < entries.len() {
            let next_start = entries[i + 1].start_line;
            trim_trailing_blanks(&lines, entry.start_line, next_start)
        } else {
            let closing = find_closing_brace_line(&lines);
            trim_trailing_blanks(&lines, entry.start_line, closing)
        };

        let entity_content = lines[entry.start_line - 1..end_line].join("\n");

        let value_content = extract_value_content(&entity_content);
        let structural_hash = Some(content_hash(value_content));

        // Build JSON Pointer path: parent_pointer + "/" + escaped_key
        let pointer = match parent_pointer {
            Some(pp) => format!("{pp}{}", entry.pointer),
            None => entry.pointer.clone(),
        };

        let abs_start = line_offset + entry.start_line - 1;
        let abs_end = line_offset + end_line - 1;

        let entity_id = build_entity_id(file_path, &entry.entity_type, &pointer, parent_entity_id);

        out.push(SemanticEntity {
            id: entity_id.clone(),
            file_path: file_path.to_string(),
            entity_type: entry.entity_type.clone(),
            name: entry.key.clone(),
            parent_id: parent_entity_id.map(str::to_string),
            content_hash: content_hash(&entity_content),
            structural_hash,
            content: entity_content.clone(),
            start_line: abs_start,
            end_line: abs_end,
            metadata: None,
        });

        // If this entry is an object, recurse into its value
        if entry.entity_type == "object" {
            if let Some(obj_str) = extract_object_value(&entity_content) {
                // The object value starts at the line with the opening `{`.
                // We need to find the absolute line of that `{` inside entity_content.
                let obj_line_in_entity = find_value_start_line(&entity_content);
                let obj_abs_line = abs_start + obj_line_in_entity - 1;
                extract_entries_recursive(
                    obj_str,
                    file_path,
                    obj_abs_line,
                    Some(&pointer),
                    Some(&entity_id),
                    out,
                );
            }
        }
    }
}

/// Given an entity content string like `  "scripts": {\n    "build": "tsc"\n  }`,
/// return a slice that starts at the opening `{` of the value and ends at (and
/// including) the matching closing `}`.
fn extract_object_value(content: &str) -> Option<&str> {
    // Skip past the first `:` (outside strings) to find the value
    let mut in_string = false;
    let mut escape_next = false;
    let mut colon_pos: Option<usize> = None;

    for (i, ch) in content.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape_next = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
        }
        if ch == ':' && !in_string {
            colon_pos = Some(i);
            break;
        }
    }

    let after_colon = &content[colon_pos? + 1..];
    // Find the opening `{`
    let brace_offset = after_colon.find('{')?;
    let obj_start = colon_pos? + 1 + brace_offset;

    // Find the matching `}`
    let mut depth = 0usize;
    in_string = false;
    escape_next = false;

    for (i, ch) in content[obj_start..].char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape_next = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if !in_string {
            match ch {
                '{' | '[' => depth += 1,
                '}' | ']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&content[obj_start..obj_start + i + 1]);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Return the 1-based line number (relative to the entity content) where the
/// object value's `{` appears.
fn find_value_start_line(content: &str) -> usize {
    let mut in_string = false;
    let mut escape_next = false;
    let mut past_colon = false;
    let mut line = 1usize;

    for ch in content.chars() {
        if ch == '\n' {
            line += 1;
            continue;
        }
        if escape_next {
            escape_next = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape_next = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if ch == ':' && !in_string {
            past_colon = true;
            continue;
        }
        if past_colon && ch == '{' {
            return line;
        }
    }
    1
}

struct JsonEntry {
    key: String,
    pointer: String,
    entity_type: String,
    start_line: usize, // 1-based, relative to the content passed in
}

/// Scan the source text to find each top-level key in the root JSON object.
/// Returns entries with accurate start_line positions (1-based, relative to `content`).
fn find_top_level_entries(content: &str) -> Vec<JsonEntry> {
    let mut entries = Vec::new();
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;
    let mut line_num: usize = 1;

    let mut current_key: Option<String> = None;
    let mut key_start = false;
    let mut key_buf = String::new();
    let mut reading_key = false;

    for ch in content.chars() {
        if ch == '\n' {
            line_num += 1;
            continue;
        }

        if escape_next {
            if reading_key {
                key_buf.push(ch);
            }
            escape_next = false;
            continue;
        }

        if ch == '\\' && in_string {
            if reading_key {
                key_buf.push(ch);
            }
            escape_next = true;
            continue;
        }

        if in_string {
            if ch == '"' {
                in_string = false;
                if reading_key {
                    reading_key = false;
                    current_key = Some(key_buf.clone());
                    key_buf.clear();
                }
            } else if reading_key {
                key_buf.push(ch);
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                if depth == 1 && current_key.is_none() && !key_start {
                    reading_key = true;
                    key_buf.clear();
                }
            }
            ':' => {
                if depth == 1 {
                    if let Some(ref key) = current_key {
                        let escaped_key = key.replace('~', "~0").replace('/', "~1");
                        let pointer = format!("/{escaped_key}");
                        entries.push(JsonEntry {
                            key: key.clone(),
                            pointer,
                            entity_type: String::new(),
                            start_line: line_num,
                        });
                        key_start = true;
                    }
                }
            }
            '{' | '[' => {
                depth += 1;
                if depth == 2 && key_start {
                    if let Some(entry) = entries.last_mut() {
                        entry.entity_type = "object".to_string();
                    }
                }
            }
            '}' | ']' => {
                depth -= 1;
            }
            ',' => {
                if depth == 1 {
                    if let Some(entry) = entries.last_mut() {
                        if entry.entity_type.is_empty() {
                            entry.entity_type = "property".to_string();
                        }
                    }
                    current_key = None;
                    key_start = false;
                }
            }
            _ => {}
        }
    }

    if let Some(entry) = entries.last_mut() {
        if entry.entity_type.is_empty() {
            entry.entity_type = "property".to_string();
        }
    }

    entries
}

/// Extract just the value portion of a `"key": value` entity content string,
/// stripping the key name so that renamed keys with identical values share the
/// same structural_hash and are detected as renames rather than delete + add.
fn extract_value_content(content: &str) -> &str {
    let mut in_string = false;
    let mut escape_next = false;
    for (i, ch) in content.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape_next = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
        }
        if ch == ':' && !in_string {
            let rest = content[i + 1..].trim();
            return rest.trim_end_matches(',').trim();
        }
    }
    content
}

/// Find the line number (1-based) of the closing `}` of the root object.
fn find_closing_brace_line(lines: &[&str]) -> usize {
    for (i, line) in lines.iter().enumerate().rev() {
        if line.trim() == "}" {
            return i + 1;
        }
    }
    lines.len()
}

/// Walk backwards from next_start to skip trailing blank lines and commas,
/// returning the end_line (1-based, inclusive) for the current entry.
fn trim_trailing_blanks(lines: &[&str], start: usize, next_start: usize) -> usize {
    let mut end = next_start - 1;
    while end > start {
        let trimmed = lines[end - 1].trim();
        if trimmed.is_empty() || trimmed == "," {
            end -= 1;
        } else {
            break;
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::change::ChangeType;
    use crate::model::identity::match_entities;

    #[test]
    fn test_json_line_positions() {
        let content = r#"{
  "name": "my-app",
  "version": "1.0.0",
  "scripts": {
    "build": "tsc",
    "test": "jest"
  },
  "description": "a test app"
}
"#;
        let plugin = JsonParserPlugin;
        let entities = plugin.extract_entities(content, "package.json");

        // Top-level entities
        let top: Vec<_> = entities.iter().filter(|e| e.parent_id.is_none()).collect();
        assert_eq!(top.len(), 4);

        assert_eq!(top[0].name, "name");
        assert_eq!(top[0].start_line, 2);
        assert_eq!(top[0].end_line, 2);

        assert_eq!(top[1].name, "version");
        assert_eq!(top[1].start_line, 3);
        assert_eq!(top[1].end_line, 3);

        assert_eq!(top[2].name, "scripts");
        assert_eq!(top[2].entity_type, "object");
        assert_eq!(top[2].start_line, 4);
        assert_eq!(top[2].end_line, 7);

        assert_eq!(top[3].name, "description");
        assert_eq!(top[3].start_line, 8);
        assert_eq!(top[3].end_line, 8);
    }

    #[test]
    fn test_nested_entities_extracted() {
        let content = r#"{
  "scripts": {
    "build": "tsc",
    "test": "jest"
  }
}
"#;
        let plugin = JsonParserPlugin;
        let entities = plugin.extract_entities(content, "package.json");

        // Should have "scripts" (top-level) + "build" and "test" (nested)
        assert_eq!(entities.len(), 3);

        let scripts = entities.iter().find(|e| e.name == "scripts").unwrap();
        assert!(scripts.parent_id.is_none());

        let build = entities.iter().find(|e| e.name == "build").unwrap();
        assert_eq!(build.parent_id, Some(scripts.id.clone()));
        assert_eq!(build.start_line, 3);

        let test = entities.iter().find(|e| e.name == "test").unwrap();
        assert_eq!(test.parent_id, Some(scripts.id.clone()));
        assert_eq!(test.start_line, 4);
    }

    #[test]
    fn test_nested_key_rename_detected() {
        // Rename "build" → "compile" inside scripts; value unchanged
        let before = r#"{
  "scripts": {
    "build": "tsc",
    "test": "jest"
  }
}
"#;
        let after = r#"{
  "scripts": {
    "compile": "tsc",
    "test": "jest"
  }
}
"#;
        let plugin = JsonParserPlugin;
        let before_entities = plugin.extract_entities(before, "package.json");
        let after_entities = plugin.extract_entities(after, "package.json");

        let result = match_entities(&before_entities, &after_entities, "package.json", None, None, None);
        let renames: Vec<_> = result
            .changes
            .iter()
            .filter(|c| c.change_type == ChangeType::Renamed)
            .collect();

        assert_eq!(renames.len(), 1, "expected exactly one rename");
        assert_eq!(renames[0].entity_name, "compile");
    }

    #[test]
    fn test_rename_detected_end_to_end() {
        let before_content = "{\n  \"timeout\": 30\n}\n";
        let after_content = "{\n  \"request_timeout\": 30\n}\n";
        let plugin = JsonParserPlugin;
        let before = plugin.extract_entities(before_content, "config.json");
        let after = plugin.extract_entities(after_content, "config.json");
        let result = match_entities(&before, &after, "config.json", None, None, None);
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].change_type, ChangeType::Renamed);
        assert_eq!(result.changes[0].entity_name, "request_timeout");
    }

    #[test]
    fn test_renamed_scalar_property_shares_structural_hash() {
        let before_content = "{\n  \"timeout\": 30\n}\n";
        let after_content = "{\n  \"request_timeout\": 30\n}\n";
        let plugin = JsonParserPlugin;
        let before = plugin.extract_entities(before_content, "config.json");
        let after = plugin.extract_entities(after_content, "config.json");
        assert_eq!(before.len(), 1);
        assert_eq!(after.len(), 1);
        assert_ne!(before[0].content_hash, after[0].content_hash);
        assert_eq!(before[0].structural_hash, after[0].structural_hash);
    }

    #[test]
    fn test_renamed_object_property_shares_structural_hash() {
        let before_content = "{\n  \"config\": {\n    \"port\": 8080\n  }\n}\n";
        let after_content = "{\n  \"settings\": {\n    \"port\": 8080\n  }\n}\n";
        let plugin = JsonParserPlugin;
        let before = plugin.extract_entities(before_content, "config.json");
        let after = plugin.extract_entities(after_content, "config.json");
        // top-level entity
        let before_top: Vec<_> = before.iter().filter(|e| e.parent_id.is_none()).collect();
        let after_top: Vec<_> = after.iter().filter(|e| e.parent_id.is_none()).collect();
        assert_eq!(before_top.len(), 1);
        assert_eq!(after_top.len(), 1);
        assert_ne!(before_top[0].content_hash, after_top[0].content_hash);
        assert_eq!(before_top[0].structural_hash, after_top[0].structural_hash);
    }
}
