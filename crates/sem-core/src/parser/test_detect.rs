//! Shared test-path detection logic.
//!
//! Single source of truth for deciding whether a file path belongs to a
//! test/fixture/benchmark directory. Used by `graph.rs` for test-entity
//! filtering.

/// Token-level matches: directory name is split on `-`, `_`, `.` and
/// any resulting token that equals one of these triggers a match.
const TEST_TOKENS: &[&str] = &["test", "tests", "spec", "specs"];

/// Exact directory-name matches for well-known dirs that don't contain
/// "test" or "spec" as a token.
const EXACT_DIR_NAMES: &[&str] = &[
    "e2e",
    "cypress",
    "playwright",
    "testing",
    "fixtures",
    "fixture",
    "benchmarks",
    "benchmark",
    "__tests__",
    "__mocks__",
];

/// File-name patterns that indicate a test file regardless of directory.
const TEST_FILE_PATTERNS: &[&str] = &["_test.", ".test.", "_spec.", ".spec."];

/// Returns `true` if any path component (directory or file stem) matches
/// built-in test heuristics.
pub fn is_test_path(path: &str) -> bool {
    is_test_path_with_custom_dirs(path, &[])
}

/// Like [`is_test_path`], but also matches if any path component equals one
/// of the caller-supplied custom directory names.
pub fn is_test_path_with_custom_dirs(path: &str, custom_dirs: &[String]) -> bool {
    let path_lower = path.to_lowercase();

    // File-name patterns (e.g. `foo_test.rs`, `bar.spec.ts`)
    if let Some(file_name) = path_lower.rsplit('/').next() {
        for pat in TEST_FILE_PATTERNS {
            if file_name.contains(pat) {
                return true;
            }
        }
    }

    // Check each path component (directory names)
    for component in path_lower.split('/') {
        if component.is_empty() {
            continue;
        }

        // Exact well-known directory names
        if EXACT_DIR_NAMES.contains(&component) {
            return true;
        }

        // Custom directory names from .semrc
        if custom_dirs
            .iter()
            .any(|d| d.eq_ignore_ascii_case(component))
        {
            return true;
        }

        // Token-based matching: split on `-`, `_`, `.` and check tokens
        let has_test_token = component
            .split(|c: char| c == '-' || c == '_' || c == '.')
            .any(|token| TEST_TOKENS.contains(&token));
        if has_test_token {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Built-in directory patterns ──────────────────────────────────────

    #[test]
    fn classic_test_dirs() {
        assert!(is_test_path("src/test/foo.ts"));
        assert!(is_test_path("src/tests/foo.ts"));
        assert!(is_test_path("src/spec/foo.ts"));
        assert!(is_test_path("src/specs/foo.ts"));
    }

    #[test]
    fn hyphenated_test_dirs() {
        assert!(is_test_path("e2e-tests/foo.ts"));
        assert!(is_test_path("integration-test/bar.py"));
        assert!(is_test_path("unit-tests/baz.js"));
    }

    #[test]
    fn underscored_test_dirs() {
        assert!(is_test_path("__tests__/baz.js"));
        assert!(is_test_path("unit_tests/foo.rs"));
        assert!(is_test_path("integration_test/bar.go"));
    }

    #[test]
    fn dotted_test_dirs() {
        assert!(is_test_path("src/test.unit/foo.ts"));
    }

    #[test]
    fn well_known_exact_dirs() {
        assert!(is_test_path("e2e/login.spec.ts"));
        assert!(is_test_path("cypress/e2e/login.spec.ts"));
        assert!(is_test_path("playwright/tests/foo.ts"));
        assert!(is_test_path("testing/helpers.py"));
        assert!(is_test_path("fixtures/data.json"));
        assert!(is_test_path("fixture/sample.txt"));
        assert!(is_test_path("benchmarks/bench_main.rs"));
        assert!(is_test_path("benchmark/perf.go"));
        assert!(is_test_path("__mocks__/api.ts"));
    }

    // ── File-name patterns ───────────────────────────────────────────────

    #[test]
    fn test_file_name_patterns() {
        assert!(is_test_path("src/utils_test.go"));
        assert!(is_test_path("src/utils.test.ts"));
        assert!(is_test_path("src/utils_spec.rb"));
        assert!(is_test_path("src/utils.spec.js"));
    }

    // ── Negative cases ───────────────────────────────────────────────────

    #[test]
    fn no_false_positives() {
        assert!(!is_test_path("src/main.rs"));
        assert!(!is_test_path("src/contest/solution.py"));
        assert!(!is_test_path("src/spectacle/viewer.ts"));
        assert!(!is_test_path("src/attestation/verify.go"));
        assert!(!is_test_path("src/latest/handler.js"));
        assert!(!is_test_path("src/protest/rally.rb"));
        assert!(!is_test_path("lib/fastest/core.ts"));
    }

    // ── Custom directories ───────────────────────────────────────────────

    #[test]
    fn custom_dir_match() {
        let custom = vec!["qa".to_string(), "smoke".to_string()];
        assert!(is_test_path_with_custom_dirs("qa/check.ts", &custom));
        assert!(is_test_path_with_custom_dirs("smoke/login.py", &custom));
    }

    #[test]
    fn custom_dir_case_insensitive() {
        let custom = vec!["QA".to_string()];
        assert!(is_test_path_with_custom_dirs("qa/check.ts", &custom));
        assert!(is_test_path_with_custom_dirs("Qa/check.ts", &custom));
    }

    #[test]
    fn custom_dir_no_false_positive() {
        let custom = vec!["qa".to_string()];
        assert!(!is_test_path_with_custom_dirs("src/main.rs", &custom));
    }

    #[test]
    fn builtin_still_works_with_custom_dirs() {
        let custom = vec!["qa".to_string()];
        assert!(is_test_path_with_custom_dirs("src/tests/foo.ts", &custom));
        assert!(is_test_path_with_custom_dirs("e2e-tests/bar.py", &custom));
    }
}
