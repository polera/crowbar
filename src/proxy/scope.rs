use parking_lot::RwLock;

pub struct Scope {
    patterns: RwLock<Vec<String>>,
}

impl Scope {
    pub fn new(patterns: Vec<String>) -> Self {
        Self {
            patterns: RwLock::new(patterns),
        }
    }

    pub fn is_in_scope(&self, host: &str) -> bool {
        let patterns = self.patterns.read();
        if patterns.is_empty() {
            return true;
        }
        let host_lower = host.to_lowercase();
        patterns.iter().any(|p| match_pattern(p, &host_lower))
    }

    pub fn patterns(&self) -> Vec<String> {
        self.patterns.read().clone()
    }

    pub fn set_patterns(&self, patterns: Vec<String>) {
        *self.patterns.write() = patterns;
    }
}

fn match_pattern(pattern: &str, host: &str) -> bool {
    let pattern = pattern.to_lowercase();
    if pattern.starts_with("*.") {
        let suffix = &pattern[1..];
        host.ends_with(suffix) || host == &pattern[2..]
    } else {
        host == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        assert!(match_pattern("example.com", "example.com"));
        assert!(!match_pattern("example.com", "other.com"));
    }

    #[test]
    fn wildcard_match() {
        assert!(match_pattern("*.example.com", "sub.example.com"));
        assert!(match_pattern("*.example.com", "example.com"));
        assert!(!match_pattern("*.example.com", "other.com"));
        assert!(match_pattern("*.example.com", "deep.sub.example.com"));
    }

    #[test]
    fn empty_scope_matches_all() {
        let scope = Scope::new(vec![]);
        assert!(scope.is_in_scope("anything.com"));
    }

    #[test]
    fn case_insensitive() {
        assert!(match_pattern("Example.COM", "example.com"));
        let scope = Scope::new(vec!["Example.COM".into()]);
        assert!(scope.is_in_scope("EXAMPLE.com"));
    }
}
