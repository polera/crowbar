pub mod persist;

use std::borrow::Cow;
use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;

use bytes::Bytes;
use regex::Regex;

fn compile_regex(pattern: &str) -> Option<Regex> {
    if pattern.is_empty() {
        return None;
    }
    Regex::new(pattern).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RuleTarget {
    Request,
    Response,
    Both,
}

impl RuleTarget {
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
    #[serde(skip)]
    compiled_regex: OnceLock<Option<Regex>>,
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
            compiled_regex: OnceLock::new(),
        }
    }

    pub fn invalidate_regex(&mut self) {
        self.compiled_regex = OnceLock::new();
    }

    fn get_compiled_regex(&self) -> Option<&Regex> {
        self.compiled_regex
            .get_or_init(|| {
                if self.is_regex {
                    compile_regex(&self.match_pattern)
                } else {
                    None
                }
            })
            .as_ref()
    }
}

pub type SharedRules = Arc<RwLock<Vec<Rule>>>;

pub fn apply_request_rules(
    rules: &SharedRules,
    uri: &mut String,
    headers: &mut [(String, String)],
    body: &mut Bytes,
) {
    let rules = rules.read();
    rules
        .iter()
        .filter(|r| r.enabled)
        .filter(|r| matches!(r.target, RuleTarget::Request | RuleTarget::Both))
        .for_each(|rule| apply_rule(rule, Some(uri), headers, body));
}

pub fn apply_response_rules(
    rules: &SharedRules,
    headers: &mut [(String, String)],
    body: &mut Bytes,
) {
    let rules = rules.read();
    rules
        .iter()
        .filter(|r| r.enabled)
        .filter(|r| matches!(r.target, RuleTarget::Response | RuleTarget::Both))
        .for_each(|rule| apply_rule(rule, None, headers, body));
}

fn apply_rule(
    rule: &Rule,
    uri: Option<&mut String>,
    headers: &mut [(String, String)],
    body: &mut Bytes,
) {
    if rule.match_pattern.is_empty() {
        return;
    }

    if rule.is_regex && rule.get_compiled_regex().is_none() {
        return;
    }

    let compiled = rule.get_compiled_regex();
    let scope = rule.scope;

    if let Some(uri) = uri
        && (scope == RuleScope::Url || scope == RuleScope::All)
        && let Cow::Owned(s) = replace_in_str(uri, &rule.match_pattern, &rule.replacement, compiled) {
            *uri = s;
        }

    if scope == RuleScope::Headers || scope == RuleScope::All {
        for (_key, value) in headers.iter_mut() {
            if let Cow::Owned(s) = replace_in_str(value, &rule.match_pattern, &rule.replacement, compiled) {
                *value = s;
            }
        }
    }

    if (scope == RuleScope::Body || scope == RuleScope::All)
        && let Ok(text) = std::str::from_utf8(body)
        && let Cow::Owned(s) = replace_in_str(text, &rule.match_pattern, &rule.replacement, compiled) {
            *body = Bytes::from(s);
        }
}

fn replace_in_str<'a>(input: &'a str, pattern: &str, replacement: &str, compiled: Option<&Regex>) -> Cow<'a, str> {
    match compiled {
        Some(re) => re.replace_all(input, replacement),
        None => {
            if input.contains(pattern) {
                Cow::Owned(input.replace(pattern, replacement))
            } else {
                Cow::Borrowed(input)
            }
        }
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
            compiled_regex: OnceLock::new(),
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
            compiled_regex: OnceLock::new(),
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
            compiled_regex: OnceLock::new(),
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
            compiled_regex: OnceLock::new(),
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
            compiled_regex: OnceLock::new(),
        }]);

        let mut uri = "https://secret.com".to_string();
        let mut headers = vec![];
        let mut body = Bytes::from("secret");

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body);
        assert_eq!(body, Bytes::from("secret"));
    }

    #[test]
    fn response_rule_applies_to_response() {
        let rules = make_rules(vec![Rule {
            name: "test".into(),
            enabled: true,
            target: RuleTarget::Response,
            scope: RuleScope::Body,
            match_pattern: "secret".into(),
            replacement: "REDACTED".into(),
            is_regex: false,
            compiled_regex: OnceLock::new(),
        }]);

        let mut headers = vec![];
        let mut body = Bytes::from("the secret data");

        apply_response_rules(&rules, &mut headers, &mut body);
        assert_eq!(body, Bytes::from("the REDACTED data"));
    }

    #[test]
    fn both_target_applies_to_request_and_response() {
        let rules = make_rules(vec![Rule {
            name: "test".into(),
            enabled: true,
            target: RuleTarget::Both,
            scope: RuleScope::Body,
            match_pattern: "token".into(),
            replacement: "MASKED".into(),
            is_regex: false,
            compiled_regex: OnceLock::new(),
        }]);

        let mut uri = "/api".to_string();
        let mut req_headers = vec![];
        let mut req_body = Bytes::from("token=abc");
        apply_request_rules(&rules, &mut uri, &mut req_headers, &mut req_body);
        assert_eq!(req_body, Bytes::from("MASKED=abc"));

        let mut resp_headers = vec![];
        let mut resp_body = Bytes::from("your token is valid");
        apply_response_rules(&rules, &mut resp_headers, &mut resp_body);
        assert_eq!(resp_body, Bytes::from("your MASKED is valid"));
    }

    #[test]
    fn scope_all_applies_everywhere() {
        let rules = make_rules(vec![Rule {
            name: "test".into(),
            enabled: true,
            target: RuleTarget::Request,
            scope: RuleScope::All,
            match_pattern: "foo".into(),
            replacement: "bar".into(),
            is_regex: false,
            compiled_regex: OnceLock::new(),
        }]);

        let mut uri = "https://example.com/foo".to_string();
        let mut headers = vec![("x-foo".into(), "foo-value".into())];
        let mut body = Bytes::from("foo content");

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body);
        assert_eq!(uri, "https://example.com/bar");
        assert_eq!(headers[0].1, "bar-value");
        assert_eq!(body, Bytes::from("bar content"));
    }

    #[test]
    fn scope_url_only_touches_url() {
        let rules = make_rules(vec![Rule {
            name: "test".into(),
            enabled: true,
            target: RuleTarget::Request,
            scope: RuleScope::Url,
            match_pattern: "v1".into(),
            replacement: "v2".into(),
            is_regex: false,
            compiled_regex: OnceLock::new(),
        }]);

        let mut uri = "/api/v1/users".to_string();
        let mut headers = vec![("x-version".into(), "v1".into())];
        let mut body = Bytes::from("version v1");

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body);
        assert_eq!(uri, "/api/v2/users");
        assert_eq!(headers[0].1, "v1");
        assert_eq!(body, Bytes::from("version v1"));
    }

    #[test]
    fn empty_pattern_is_noop() {
        let rules = make_rules(vec![Rule {
            name: "test".into(),
            enabled: true,
            target: RuleTarget::Both,
            scope: RuleScope::All,
            match_pattern: "".into(),
            replacement: "something".into(),
            is_regex: false,
            compiled_regex: OnceLock::new(),
        }]);

        let mut uri = "/api".to_string();
        let mut headers = vec![];
        let mut body = Bytes::from("body");

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body);
        assert_eq!(uri, "/api");
        assert_eq!(body, Bytes::from("body"));
    }

    #[test]
    fn invalid_regex_is_noop() {
        let rules = make_rules(vec![Rule {
            name: "test".into(),
            enabled: true,
            target: RuleTarget::Request,
            scope: RuleScope::All,
            match_pattern: "[invalid".into(),
            replacement: "x".into(),
            is_regex: true,
            compiled_regex: OnceLock::new(),
        }]);

        let mut uri = "/api".to_string();
        let mut headers = vec![];
        let mut body = Bytes::from("body");

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body);
        assert_eq!(body, Bytes::from("body"));
    }

    #[test]
    fn regex_replace_all_occurrences() {
        let rules = make_rules(vec![Rule {
            name: "test".into(),
            enabled: true,
            target: RuleTarget::Request,
            scope: RuleScope::Body,
            match_pattern: r"\d+".into(),
            replacement: "N".into(),
            is_regex: true,
            compiled_regex: OnceLock::new(),
        }]);

        let mut uri = "/api".to_string();
        let mut headers = vec![];
        let mut body = Bytes::from("id=123&count=456");

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body);
        assert_eq!(body, Bytes::from("id=N&count=N"));
    }

    #[test]
    fn rule_new_defaults() {
        let rule = Rule::new("test rule".into());
        assert!(rule.enabled);
        assert_eq!(rule.target, RuleTarget::Both);
        assert_eq!(rule.scope, RuleScope::All);
        assert!(rule.match_pattern.is_empty());
        assert!(!rule.is_regex);
    }

    #[test]
    fn rule_target_cycle() {
        assert_eq!(RuleTarget::Request.next(), RuleTarget::Response);
        assert_eq!(RuleTarget::Response.next(), RuleTarget::Both);
        assert_eq!(RuleTarget::Both.next(), RuleTarget::Request);
    }

    #[test]
    fn rule_scope_cycle() {
        assert_eq!(RuleScope::Url.next(), RuleScope::Headers);
        assert_eq!(RuleScope::Headers.next(), RuleScope::Body);
        assert_eq!(RuleScope::Body.next(), RuleScope::All);
        assert_eq!(RuleScope::All.next(), RuleScope::Url);
    }

    #[test]
    fn rule_target_labels() {
        assert_eq!(RuleTarget::Request.label(), "Request");
        assert_eq!(RuleTarget::Response.label(), "Response");
        assert_eq!(RuleTarget::Both.label(), "Both");
    }

    #[test]
    fn rule_scope_labels() {
        assert_eq!(RuleScope::Url.label(), "URL");
        assert_eq!(RuleScope::Headers.label(), "Headers");
        assert_eq!(RuleScope::Body.label(), "Body");
        assert_eq!(RuleScope::All.label(), "All");
    }

    #[test]
    fn invalidate_regex_allows_recompile() {
        let mut rule = Rule {
            name: "test".into(),
            enabled: true,
            target: RuleTarget::Request,
            scope: RuleScope::Body,
            match_pattern: r"\d+".into(),
            replacement: "N".into(),
            is_regex: true,
            compiled_regex: OnceLock::new(),
        };

        assert!(rule.get_compiled_regex().is_some());

        rule.match_pattern = r"\w+".into();
        rule.invalidate_regex();
        let re = rule.get_compiled_regex().unwrap();
        assert!(re.is_match("hello"));
    }

    #[test]
    fn multiple_rules_apply_in_order() {
        let rules = make_rules(vec![
            Rule {
                name: "first".into(),
                enabled: true,
                target: RuleTarget::Request,
                scope: RuleScope::Body,
                match_pattern: "aaa".into(),
                replacement: "bbb".into(),
                is_regex: false,
                compiled_regex: OnceLock::new(),
            },
            Rule {
                name: "second".into(),
                enabled: true,
                target: RuleTarget::Request,
                scope: RuleScope::Body,
                match_pattern: "bbb".into(),
                replacement: "ccc".into(),
                is_regex: false,
                compiled_regex: OnceLock::new(),
            },
        ]);

        let mut uri = "/api".to_string();
        let mut headers = vec![];
        let mut body = Bytes::from("aaa");

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body);
        assert_eq!(body, Bytes::from("ccc"));
    }
}
