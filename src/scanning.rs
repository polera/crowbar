use crate::http::models::{RequestData, ResponseData};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Info => "INFO",
            Severity::Low => "LOW",
            Severity::Medium => "MED",
            Severity::High => "HIGH",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub title: String,
    pub detail: String,
}

pub fn scan_response(request: &RequestData, response: &ResponseData) -> Vec<Finding> {
    let mut findings = Vec::new();

    check_security_headers(request, response, &mut findings);
    check_info_disclosure(response, &mut findings);
    check_cookie_flags(request, response, &mut findings);
    check_status_code(response, &mut findings);
    check_body_patterns(response, &mut findings);

    findings
}

fn has_header(headers: &[(String, String)], name: &str) -> bool {
    headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
}

fn get_headers<'a>(headers: &'a [(String, String)], name: &str) -> Vec<&'a str> {
    headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
        .collect()
}

fn check_security_headers(
    request: &RequestData,
    response: &ResponseData,
    findings: &mut Vec<Finding>,
) {
    if request.is_tls && !has_header(&response.headers, "strict-transport-security") {
        findings.push(Finding {
            severity: Severity::Medium,
            title: "Missing Strict-Transport-Security".into(),
            detail: "HSTS header not set. Clients may connect over plain HTTP.".into(),
        });
    }

    if !has_header(&response.headers, "content-security-policy") {
        findings.push(Finding {
            severity: Severity::Low,
            title: "Missing Content-Security-Policy".into(),
            detail: "No CSP header. XSS risk is elevated without a content policy.".into(),
        });
    }

    if !has_header(&response.headers, "x-frame-options") {
        findings.push(Finding {
            severity: Severity::Low,
            title: "Missing X-Frame-Options".into(),
            detail: "Page can be framed by any origin (clickjacking risk).".into(),
        });
    }

    if !has_header(&response.headers, "x-content-type-options") {
        findings.push(Finding {
            severity: Severity::Low,
            title: "Missing X-Content-Type-Options".into(),
            detail: "Browser may MIME-sniff the response, risking content confusion.".into(),
        });
    }
}

fn check_info_disclosure(response: &ResponseData, findings: &mut Vec<Finding>) {
    for val in get_headers(&response.headers, "server") {
        findings.push(Finding {
            severity: Severity::Info,
            title: "Server header disclosed".into(),
            detail: format!("Server: {}", val),
        });
    }

    for val in get_headers(&response.headers, "x-powered-by") {
        findings.push(Finding {
            severity: Severity::Info,
            title: "X-Powered-By header disclosed".into(),
            detail: format!("X-Powered-By: {}", val),
        });
    }
}

fn check_cookie_flags(
    request: &RequestData,
    response: &ResponseData,
    findings: &mut Vec<Finding>,
) {
    for cookie in get_headers(&response.headers, "set-cookie") {
        let lower = cookie.to_lowercase();

        if request.is_tls && !lower.contains("secure") {
            findings.push(Finding {
                severity: Severity::Medium,
                title: "Cookie missing Secure flag".into(),
                detail: format!("Set-Cookie without Secure: {}", truncate_cookie(cookie)),
            });
        }

        if !lower.contains("httponly") {
            findings.push(Finding {
                severity: Severity::Medium,
                title: "Cookie missing HttpOnly flag".into(),
                detail: format!("Set-Cookie without HttpOnly: {}", truncate_cookie(cookie)),
            });
        }

        if !lower.contains("samesite") {
            findings.push(Finding {
                severity: Severity::Low,
                title: "Cookie missing SameSite attribute".into(),
                detail: format!("Set-Cookie without SameSite: {}", truncate_cookie(cookie)),
            });
        }
    }
}

fn check_status_code(response: &ResponseData, findings: &mut Vec<Finding>) {
    if response.status >= 500 {
        findings.push(Finding {
            severity: Severity::Info,
            title: format!("Server error: {} {}", response.status, response.reason),
            detail: "5xx response may indicate application errors or misconfigurations.".into(),
        });
    }
}

fn check_body_patterns(response: &ResponseData, findings: &mut Vec<Finding>) {
    let body = match std::str::from_utf8(&response.body) {
        Ok(t) => t,
        Err(_) => return,
    };

    let trace_patterns = [
        "at java.",
        "at sun.",
        "Traceback (most recent call last)",
        "stack trace:",
        "System.NullReferenceException",
        "Microsoft.AspNetCore",
        "goroutine ",
        "panic: runtime error",
    ];

    for pattern in &trace_patterns {
        if body.contains(pattern) {
            findings.push(Finding {
                severity: Severity::High,
                title: "Stack trace in response".into(),
                detail: format!("Detected pattern: \"{}\"", pattern),
            });
            break;
        }
    }
}

fn truncate_cookie(cookie: &str) -> String {
    if cookie.len() > 60 {
        format!("{}...", &cookie[..57])
    } else {
        cookie.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::time::{Duration, SystemTime};
    use crate::http::models::{HttpVersion, RequestId};

    fn make_request(is_tls: bool) -> RequestData {
        RequestData {
            id: RequestId(1),
            method: "GET".into(),
            uri: "https://example.com/".into(),
            host: "example.com".into(),
            version: HttpVersion::Http11,
            headers: vec![],
            body: Bytes::new(),
            is_tls,
            is_grpc: false,
            timestamp: SystemTime::now(),
        }
    }

    fn make_response(headers: Vec<(&str, &str)>, body: &str) -> ResponseData {
        ResponseData {
            status: 200,
            reason: "OK".into(),
            version: HttpVersion::Http11,
            headers: headers.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
            body: Bytes::from(body.to_string()),
            trailers: Vec::new(),
            duration: Duration::from_millis(50),
        }
    }

    #[test]
    fn missing_hsts_on_tls() {
        let req = make_request(true);
        let resp = make_response(vec![], "");
        let findings = scan_response(&req, &resp);
        assert!(findings.iter().any(|f| f.title.contains("Strict-Transport-Security")));
    }

    #[test]
    fn hsts_not_flagged_on_http() {
        let req = make_request(false);
        let resp = make_response(vec![], "");
        let findings = scan_response(&req, &resp);
        assert!(!findings.iter().any(|f| f.title.contains("Strict-Transport-Security")));
    }

    #[test]
    fn server_header_disclosure() {
        let req = make_request(false);
        let resp = make_response(vec![("Server", "Apache/2.4.52")], "");
        let findings = scan_response(&req, &resp);
        assert!(findings.iter().any(|f| f.title.contains("Server header")));
    }

    #[test]
    fn cookie_missing_flags() {
        let req = make_request(true);
        let resp = make_response(vec![("Set-Cookie", "session=abc; Path=/")], "");
        let findings = scan_response(&req, &resp);
        assert!(findings.iter().any(|f| f.title.contains("Secure")));
        assert!(findings.iter().any(|f| f.title.contains("HttpOnly")));
        assert!(findings.iter().any(|f| f.title.contains("SameSite")));
    }

    #[test]
    fn stack_trace_detection() {
        let req = make_request(false);
        let resp = make_response(vec![], "Error: Traceback (most recent call last):\n  File app.py");
        let findings = scan_response(&req, &resp);
        assert!(findings.iter().any(|f| f.severity == Severity::High));
    }
}
