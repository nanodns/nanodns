/// Match `name` against a wildcard pattern.
///
/// Rules (matching the original Python NanoDNS spec):
///   - Exact pattern `"foo.bar"` → only matches `"foo.bar"`
///   - Wildcard `"*.foo.bar"` → matches **one level** of subdomains only:
///     `any.foo.bar` ✅   `foo.bar` ✅   `a.b.foo.bar` ❌
///
/// Comparisons are case-insensitive.
pub fn matches(pattern: &str, name: &str) -> bool {
    let pattern = pattern.trim_end_matches('.').to_lowercase();
    let name = name.trim_end_matches('.').to_lowercase();

    if !pattern.starts_with("*.") {
        // Exact match only
        return pattern == name;
    }

    // *.foo.bar  →  suffix = "foo.bar"
    let suffix = &pattern[2..]; // strip leading "*."

    // Direct match of the bare zone apex: "foo.bar" == suffix
    if name == suffix {
        return true;
    }

    // One-level subdomain: name must be "<single-label>.<suffix>"
    // i.e. strip the suffix plus the dot, then ensure no dot remains in the prefix
    if let Some(prefix) = name.strip_suffix(&format!(".{}", suffix)) {
        return !prefix.contains('.');
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        assert!(matches("foo.bar", "foo.bar"));
        assert!(!matches("foo.bar", "baz.bar"));
        assert!(!matches("foo.bar", "sub.foo.bar"));
    }

    #[test]
    fn wildcard_single_level() {
        assert!(matches("*.foo.bar", "any.foo.bar"), "direct sub");
        assert!(matches("*.foo.bar", "foo.bar"), "apex itself");
        assert!(
            !matches("*.foo.bar", "a.b.foo.bar"),
            "two levels deep — should NOT match"
        );
        assert!(!matches("*.foo.bar", "other.com"), "unrelated domain");
    }

    #[test]
    fn case_insensitive() {
        assert!(matches("*.FOO.BAR", "Any.foo.bar"));
        assert!(matches("EXACT.TEST", "exact.test"));
    }
}
