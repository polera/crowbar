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
    match_pattern: String,
    pub replacement: String,
    is_regex: bool,
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

    fn invalidate_regex(&mut self) {
        self.compiled_regex = OnceLock::new();
    }

    pub fn pattern(&self) -> &str {
        &self.match_pattern
    }
    pub fn is_regex(&self) -> bool {
        self.is_regex
    }

    pub fn validate(&self) -> Result<(), regex::Error> {
        if self.is_regex && !self.match_pattern.is_empty() {
            Regex::new(&self.match_pattern)?;
        }
        Ok(())
    }

    pub fn set_pattern(&mut self, pattern: String) -> Result<(), regex::Error> {
        if self.is_regex && !pattern.is_empty() {
            Regex::new(&pattern)?;
        }
        self.match_pattern = pattern;
        self.invalidate_regex();
        Ok(())
    }

    pub fn set_regex(&mut self, enabled: bool) -> Result<(), regex::Error> {
        if enabled && !self.match_pattern.is_empty() {
            Regex::new(&self.match_pattern)?;
        }
        self.is_regex = enabled;
        self.invalidate_regex();
        Ok(())
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

#[derive(Debug, thiserror::Error)]
#[error("Rewritten value exceeds configured limit")]
pub struct RewriteLimitExceeded;

pub fn apply_request_rules(
    rules: &SharedRules,
    uri: &mut String,
    headers: &mut [(String, String)],
    body: &mut Bytes,
    max_bytes: usize,
) -> Result<(), RewriteLimitExceeded> {
    let rules = rules.read();
    for rule in rules
        .iter()
        .filter(|r| r.enabled)
        .filter(|r| matches!(r.target, RuleTarget::Request | RuleTarget::Both))
    {
        apply_rule(rule, Some(uri), headers, body, max_bytes)?;
    }
    Ok(())
}

pub fn apply_response_rules(
    rules: &SharedRules,
    headers: &mut [(String, String)],
    body: &mut Bytes,
    max_bytes: usize,
) -> Result<(), RewriteLimitExceeded> {
    let rules = rules.read();
    for rule in rules
        .iter()
        .filter(|r| r.enabled)
        .filter(|r| matches!(r.target, RuleTarget::Response | RuleTarget::Both))
    {
        apply_rule(rule, None, headers, body, max_bytes)?;
    }
    Ok(())
}

fn apply_rule(
    rule: &Rule,
    uri: Option<&mut String>,
    headers: &mut [(String, String)],
    body: &mut Bytes,
    max_bytes: usize,
) -> Result<(), RewriteLimitExceeded> {
    if rule.match_pattern.is_empty() {
        return Ok(());
    }

    if rule.is_regex && rule.get_compiled_regex().is_none() {
        return Ok(());
    }

    let compiled = rule.get_compiled_regex();
    let scope = rule.scope;

    if let Some(uri) = uri
        && (scope == RuleScope::Url || scope == RuleScope::All)
        && let Cow::Owned(s) = replace_in_str(
            uri,
            &rule.match_pattern,
            &rule.replacement,
            compiled,
            max_bytes,
        )?
    {
        *uri = s;
    }

    if scope == RuleScope::Headers || scope == RuleScope::All {
        for (_key, value) in headers.iter_mut() {
            if let Cow::Owned(s) = replace_in_str(
                value,
                &rule.match_pattern,
                &rule.replacement,
                compiled,
                max_bytes,
            )? {
                *value = s;
            }
        }
    }

    if (scope == RuleScope::Body || scope == RuleScope::All)
        && let Ok(text) = std::str::from_utf8(body)
        && let Cow::Owned(s) = replace_in_str(
            text,
            &rule.match_pattern,
            &rule.replacement,
            compiled,
            max_bytes,
        )?
    {
        *body = Bytes::from(s);
    }
    Ok(())
}

fn replace_in_str<'a>(
    input: &'a str,
    pattern: &str,
    replacement: &str,
    compiled: Option<&Regex>,
    max_bytes: usize,
) -> Result<Cow<'a, str>, RewriteLimitExceeded> {
    let mut output = String::new();
    let mut end = 0;
    let mut matched = false;
    match compiled {
        Some(re) => {
            for captures in re.captures_iter(input) {
                let found = captures.get(0).expect("whole match exists");
                append_bounded(&mut output, &input[end..found.start()], max_bytes)?;
                expand_bounded(&mut output, replacement, &captures, max_bytes)?;
                end = found.end();
                matched = true;
            }
        }
        None => {
            for (start, found) in input.match_indices(pattern) {
                append_bounded(&mut output, &input[end..start], max_bytes)?;
                append_bounded(&mut output, replacement, max_bytes)?;
                end = start + found.len();
                matched = true;
            }
        }
    }
    if !matched {
        return Ok(Cow::Borrowed(input));
    }
    append_bounded(&mut output, &input[end..], max_bytes)?;
    Ok(Cow::Owned(output))
}

fn append_bounded(
    output: &mut String,
    text: &str,
    max_bytes: usize,
) -> Result<(), RewriteLimitExceeded> {
    let size = output
        .len()
        .checked_add(text.len())
        .filter(|&size| size <= max_bytes)
        .ok_or(RewriteLimitExceeded)?;
    if size > output.capacity() {
        // Avoid String's automatic growth reserving more than the configured cap.
        let capacity = size.max(output.capacity().saturating_mul(2)).min(max_bytes);
        output.reserve_exact(capacity - output.len());
    }
    output.push_str(text);
    Ok(())
}

/// Match regex's $name, ${name}, numeric references and $$ semantics without
/// allocating an unbounded intermediate string for capture expansion.
fn expand_bounded(
    output: &mut String,
    mut replacement: &str,
    captures: &regex::Captures<'_>,
    max_bytes: usize,
) -> Result<(), RewriteLimitExceeded> {
    while let Some(dollar) = replacement.find('$') {
        append_bounded(output, &replacement[..dollar], max_bytes)?;
        replacement = &replacement[dollar + 1..];
        if let Some(rest) = replacement.strip_prefix('$') {
            append_bounded(output, "$", max_bytes)?;
            replacement = rest;
            continue;
        }
        let reference = if let Some(braced) = replacement.strip_prefix('{') {
            braced.find('}').map(|end| (&braced[..end], end + 2))
        } else {
            let end = replacement
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
            (end > 0).then_some((&replacement[..end], end))
        };
        if let Some((name, consumed)) = reference {
            let capture = match name.parse::<usize>() {
                Ok(index) => captures.get(index),
                Err(_) => captures.name(name),
            };
            if let Some(capture) = capture {
                append_bounded(output, capture.as_str(), max_bytes)?;
            }
            replacement = &replacement[consumed..];
        } else {
            append_bounded(output, "$", max_bytes)?;
        }
    }
    append_bounded(output, replacement, max_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    proptest::proptest! {
        #[test]
        fn bounded_regex_replacement_matches_regex_crate(
            input in "[abcβ]{0,32}",
            replacement in "[a-zA-Z0-9_${}β]{0,64}",
        ) {
            let re = Regex::new(r"(a*)(?P<word>[bβ]*)(c?)").unwrap();
            let expected = re.replace_all(&input, replacement.as_str());
            let actual = replace_in_str(&input, "unused", &replacement, Some(&re), 1024 * 1024).unwrap();
            proptest::prop_assert_eq!(actual, expected);
        }
    }

    #[test]
    fn literal_and_regex_expansion_stop_before_exceeding_limit() {
        for compiled in [None, Some(Regex::new("a").unwrap())] {
            assert!(replace_in_str("aaa", "a", "1234", compiled.as_ref(), 8).is_err());
            assert_eq!(
                replace_in_str("aa", "a", "1234", compiled.as_ref(), 8).unwrap(),
                "12341234"
            );
        }
        let mut output = String::new();
        assert!(append_bounded(&mut output, "oversized", 2).is_err());
        assert_eq!(output.capacity(), 0);
    }

    #[test]
    fn regex_capture_expansion_is_bounded_and_preserves_regex_semantics() {
        let re = Regex::new(r"(?P<word>\w+)(?:-(\d+))?").unwrap();
        for replacement in [
            "$word",
            "${word}:$2",
            "$0$0",
            "$$$word",
            "$unknown",
            "${}",
            "${word",
            "$",
            "$1suffix",
            "${1}suffix",
            "${名}",
            "$$$0$$",
        ] {
            let expected = re.replace_all("alpha-12 βeta", replacement);
            assert_eq!(
                replace_in_str("alpha-12 βeta", "unused", replacement, Some(&re), 1024).unwrap(),
                expected
            );
        }
        let re = Regex::new(r"(?s)(.*)").unwrap();
        assert!(replace_in_str("abcd", "", "$1$1$1", Some(&re), 8).is_err());
        let re = Regex::new("").unwrap();
        assert_eq!(replace_in_str("é", "", "x", Some(&re), 4).unwrap(), "xéx");
    }

    #[test]
    fn chained_rules_cannot_create_oversized_intermediate_bodies() {
        let mut rule = Rule::new("expand".into());
        rule.scope = RuleScope::Body;
        rule.set_pattern("a".into()).unwrap();
        rule.replacement = "a".repeat(16);
        let rules = make_rules(vec![rule.clone(), rule.clone(), rule]);
        let mut body = Bytes::from(vec![b'a'; 8192]);
        assert!(
            apply_request_rules(
                &rules,
                &mut String::new(),
                &mut [],
                &mut body,
                2 * 1024 * 1024
            )
            .is_err()
        );
        assert_eq!(body.len(), 2 * 1024 * 1024);
        let mut body = Bytes::from(vec![b'a'; 8192]);
        assert!(apply_response_rules(&rules, &mut [], &mut body, 8192).is_err());
        assert_eq!(body.len(), 8192);
    }

    #[test]
    fn url_and_header_rewrites_are_bounded_too() {
        let mut rule = Rule::new("expand".into());
        rule.set_pattern("a".into()).unwrap();
        rule.replacement = "long".into();
        let rules = make_rules(vec![rule]);
        assert!(
            apply_request_rules(&rules, &mut "aaa".into(), &mut [], &mut Bytes::new(), 8).is_err()
        );
        assert!(
            apply_response_rules(
                &rules,
                &mut [("x".into(), "aaa".into())],
                &mut Bytes::new(),
                8
            )
            .is_err()
        );
    }

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

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body, 1024).unwrap();
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

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body, 1024).unwrap();
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

        apply_response_rules(&rules, &mut headers, &mut body, 1024).unwrap();
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

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body, 1024).unwrap();
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

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body, 1024).unwrap();
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

        apply_response_rules(&rules, &mut headers, &mut body, 1024).unwrap();
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
        apply_request_rules(&rules, &mut uri, &mut req_headers, &mut req_body, 1024).unwrap();
        assert_eq!(req_body, Bytes::from("MASKED=abc"));

        let mut resp_headers = vec![];
        let mut resp_body = Bytes::from("your token is valid");
        apply_response_rules(&rules, &mut resp_headers, &mut resp_body, 1024).unwrap();
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

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body, 1024).unwrap();
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

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body, 1024).unwrap();
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

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body, 1024).unwrap();
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

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body, 1024).unwrap();
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

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body, 1024).unwrap();
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

        apply_request_rules(&rules, &mut uri, &mut headers, &mut body, 1024).unwrap();
        assert_eq!(body, Bytes::from("ccc"));
    }
}
