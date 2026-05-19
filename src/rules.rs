use std::sync::{Arc, RwLock};

use bytes::Bytes;
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RuleTarget {
    Request,
    Response,
    Both,
}

impl RuleTarget {
    pub const ALL: [RuleTarget; 3] = [RuleTarget::Request, RuleTarget::Response, RuleTarget::Both];

    pub fn label(self) -> &'static str {
        match self {
            RuleTarget::Request => "Request",
            RuleTarget::Response => "Response",
            RuleTarget::Both => "Both",
        }
    }

    pub fn next(self) -> Self {
        match self {
            RuleTarget::Request => RuleTarget::Response,
            RuleTarget::Response => RuleTarget::Both,
            RuleTarget::Both => RuleTarget::Request,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RuleScope {
    Url,
    Headers,
    Body,
    All,
}

impl RuleScope {
    pub const ALL: [RuleScope; 4] = [RuleScope::Url, RuleScope::Headers, RuleScope::Body, RuleScope::All];

    pub fn label(self) -> &'static str {
        match self {
            RuleScope::Url => "URL",
            RuleScope::Headers => "Headers",
            RuleScope::Body => "Body",
            RuleScope::All => "All",
        }
    }

    pub fn next(self) -> Self {
        match self {
            RuleScope::Url => RuleScope::Headers,
            RuleScope::Headers => RuleScope::Body,
            RuleScope::Body => RuleScope::All,
            RuleScope::All => RuleScope::Url,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Rule {
    pub name: String,
    pub enabled: bool,
    pub target: RuleTarget,
    pub scope: RuleScope,
    pub match_pattern: String,
    pub replacement: String,
    pub is_regex: bool,
}

impl Rule {
    pub fn new(name: String) -> Self {
        Self {
            name,
            enabled: true,
            target: RuleTarget::Both,
            scope: RuleScope::All,
            match_pattern: String::new(),
            replacement: String::new(),
            is_regex: false,
        }
    }
}

pub type SharedRules = Arc<RwLock<Vec<Rule>>>;

pub fn apply_request_rules(
    rules: &SharedRules,
    uri: &mut String,
    headers: &mut Vec<(String, String)>,
    body: &mut Bytes,
) {
    let rules = rules.read().unwrap();
    for rule in rules.iter() {
        if !rule.enabled {
            continue;
        }
        if rule.target != RuleTarget::Request && rule.target != RuleTarget::Both {
            continue;
        }
        apply_rule(rule, Some(uri), headers, body);
    }
}

pub fn apply_response_rules(
    rules: &SharedRules,
    headers: &mut Vec<(String, String)>,
    body: &mut Bytes,
) {
    let rules = rules.read().unwrap();
    for rule in rules.iter() {
        if !rule.enabled {
            continue;
        }
        if rule.target != RuleTarget::Response && rule.target != RuleTarget::Both {
            continue;
        }
        apply_rule(rule, None, headers, body);
    }
}

fn apply_rule(
    rule: &Rule,
    uri: Option<&mut String>,
    headers: &mut Vec<(String, String)>,
    body: &mut Bytes,
) {
    if rule.match_pattern.is_empty() {
        return;
    }

    let scope = rule.scope;

    if let Some(uri) = uri {
        if scope == RuleScope::Url || scope == RuleScope::All {
            *uri = replace_in_str(uri, &rule.match_pattern, &rule.replacement, rule.is_regex);
        }
    }

    if scope == RuleScope::Headers || scope == RuleScope::All {
        for (_key, value) in headers.iter_mut() {
            *value = replace_in_str(value, &rule.match_pattern, &rule.replacement, rule.is_regex);
        }
    }

    if scope == RuleScope::Body || scope == RuleScope::All {
        if let Ok(text) = std::str::from_utf8(body) {
            let replaced = replace_in_str(text, &rule.match_pattern, &rule.replacement, rule.is_regex);
            if replaced != text {
                *body = Bytes::from(replaced);
            }
        }
    }
}

fn replace_in_str(input: &str, pattern: &str, replacement: &str, is_regex: bool) -> String {
    if is_regex {
        match Regex::new(pattern) {
            Ok(re) => re.replace_all(input, replacement).to_string(),
            Err(_) => input.to_string(),
        }
    } else {
        input.replace(pattern, replacement)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rules(rules: Vec<Rule>) -> SharedRules {
        Arc::new(RwLock::new(rules))
    }

    #[test]
    fn literal_replace_in_body() {
        let rules = make_rules(vec![Rule {
            name: "test".into(),
            enabled: true,
            target: RuleTarget::Request,
            scope: RuleScope::Body,
            match_pattern: "secret".into(),
            replacement: "REDACTED".into(),
            is_regex: false,
        }]);

        let mut uri = "https://example.com".to_string();
        let mut headers = vec![("content-type".into(), "text/plain".into())];
        let mut body = Bytes::from("the secret value");

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body);
        assert_eq!(body, Bytes::from("the REDACTED value"));
        assert_eq!(uri, "https://example.com");
    }

    #[test]
    fn regex_replace_in_url() {
        let rules = make_rules(vec![Rule {
            name: "test".into(),
            enabled: true,
            target: RuleTarget::Request,
            scope: RuleScope::Url,
            match_pattern: r"token=\w+".into(),
            replacement: "token=MASKED".into(),
            is_regex: true,
        }]);

        let mut uri = "https://example.com/api?token=abc123&other=1".to_string();
        let mut headers = vec![];
        let mut body = Bytes::new();

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body);
        assert_eq!(uri, "https://example.com/api?token=MASKED&other=1");
    }

    #[test]
    fn replace_in_headers() {
        let rules = make_rules(vec![Rule {
            name: "test".into(),
            enabled: true,
            target: RuleTarget::Response,
            scope: RuleScope::Headers,
            match_pattern: "Apache/2.4".into(),
            replacement: "Hidden".into(),
            is_regex: false,
        }]);

        let mut headers = vec![
            ("content-type".into(), "text/html".into()),
            ("server".into(), "Apache/2.4".into()),
        ];
        let mut body = Bytes::from("Apache/2.4 should not change");

        apply_response_rules(&rules, &mut headers, &mut body);
        assert_eq!(headers[1].1, "Hidden");
        assert_eq!(body, Bytes::from("Apache/2.4 should not change"));
    }

    #[test]
    fn disabled_rule_skipped() {
        let rules = make_rules(vec![Rule {
            name: "test".into(),
            enabled: false,
            target: RuleTarget::Both,
            scope: RuleScope::All,
            match_pattern: "secret".into(),
            replacement: "REDACTED".into(),
            is_regex: false,
        }]);

        let mut uri = "https://secret.com".to_string();
        let mut headers = vec![];
        let mut body = Bytes::from("secret");

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body);
        assert_eq!(uri, "https://secret.com");
        assert_eq!(body, Bytes::from("secret"));
    }

    #[test]
    fn wrong_target_skipped() {
        let rules = make_rules(vec![Rule {
            name: "test".into(),
            enabled: true,
            target: RuleTarget::Response,
            scope: RuleScope::All,
            match_pattern: "secret".into(),
            replacement: "REDACTED".into(),
            is_regex: false,
        }]);

        let mut uri = "https://secret.com".to_string();
        let mut headers = vec![];
        let mut body = Bytes::from("secret");

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body);
        assert_eq!(body, Bytes::from("secret"));
    }
}
