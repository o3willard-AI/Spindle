#![allow(warnings)]
/// Pattern-based secret scanning for log lines.

use regex::Regex;

fn build_patterns() -> Vec<Regex> {
    vec![
        Regex::new(r"(?i)password\\s*=\\s*\\S+").unwrap(),
        Regex::new(r"(?i)secret\\s*=\\s*\\S+").unwrap(),
        Regex::new(r"(?i)token\\s*=\\s*Bearer\\s+\\S+").unwrap(),
        Regex::new(r"(?i)token\\s*=\\s*eyJ[A-Za-z0-9_-]+\\.eyJ[A-Za-z0-9_-]+\\.eyJ[A-Za-z0-9_-]+"  ).unwrap(),
        Regex::new(r"(?i)api_key\\s*=\\s*\\S+").unwrap(),
        Regex::new(r"(?i)access_token\\s*=\\s*\\S+").unwrap(),
        Regex::new(r"(?i)apikey\\s*=\\s*\\S+").unwrap(),
        // Quoted values: password="..." or token='...'
        Regex::new(r#"(?i)password\s*=\s*['\"][^'\"]+['\"]"#).unwrap(),
        Regex::new(r#"(?i)token\s*=\s*['\"][^'\"]+['\"]"#).unwrap(),
    ]
}

pub struct ScanResult {
    pub original: String,
    pub redacted: String,
    pub secrets_found: bool,
}

pub fn scan_log_line(line: &str) -> ScanResult {
    let patterns = build_patterns();
    let mut result = ScanResult {
        original: line.to_string(),
        redacted: line.to_string(),
        secrets_found: false,
    };
    for pattern in &patterns {
        if let Some(caps) = pattern.captures(line) {
            let start = caps.get(0).unwrap().start();
            let end = caps.get(0).unwrap().end();
            result.redacted = format!(
                "{}{}{}",
                &result.redacted[..start],
                "[REDACTED]",
                &result.redacted[end..]
            );
            result.secrets_found = true;
        }
    }
    result
}
