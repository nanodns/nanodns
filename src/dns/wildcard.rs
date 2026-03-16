/// Check if `name` matches a wildcard pattern like `*.foo.bar`.
/// Also handles exact matches.
pub fn matches(pattern: &str, name: &str) -> bool {
    let pattern = pattern.trim_end_matches('.').to_lowercase();
    let name = name.trim_end_matches('.').to_lowercase();

    if !pattern.starts_with("*.") {
        return pattern == name;
    }

    // *.foo.bar should match any.foo.bar, deep.any.foo.bar, etc.
    let suffix = &pattern[1..]; // ".foo.bar"
    name.ends_with(suffix) || name == &suffix[1..] // also match "foo.bar" itself
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        assert!(matches("foo.bar", "foo.bar"));
        assert!(!matches("foo.bar", "baz.bar"));
    }

    #[test]
    fn wildcard_match() {
        assert!(matches("*.foo.bar", "any.foo.bar"));
        assert!(matches("*.foo.bar", "deep.any.foo.bar"));
        assert!(matches("*.foo.bar", "foo.bar"));
        assert!(!matches("*.foo.bar", "other.com"));
    }
}
