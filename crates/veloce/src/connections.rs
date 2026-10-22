//! Saved remote connections: which kinds exist, what each one asks for, and how a
//! form is validated and stored.
//!
//! The backends themselves arrive in a later milestone; this is the part Settings
//! needs now so connections can be added, listed and removed in one place.
//! Secrets (SSH passwords, S3 secret keys) are deliberately never part of what is
//! stored: they will live in the operating system's keychain.

use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ssh,
    S3,
    WebDav,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Ssh, Kind::S3, Kind::WebDav];

    /// Stored in the database, so these strings must never change.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Ssh => "ssh",
            Kind::S3 => "s3",
            Kind::WebDav => "webdav",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Ssh => "SSH / SFTP",
            Kind::S3 => "S3 / object storage",
            Kind::WebDav => "WebDAV",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }
}

/// One input in a connection form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub placeholder: &'static str,
    pub required: bool,
}

const fn field(
    key: &'static str,
    label: &'static str,
    placeholder: &'static str,
    required: bool,
) -> Field {
    Field {
        key,
        label,
        placeholder,
        required,
    }
}

const SSH_FIELDS: &[Field] = &[
    field("host", "Host", "server.example.com", true),
    field("port", "Port", "22", false),
    field("user", "User", "deploy", true),
    field("key_path", "Private key", "~/.ssh/id_ed25519", false),
];

const S3_FIELDS: &[Field] = &[
    field("bucket", "Bucket", "my-bucket", true),
    field("region", "Region", "us-east-1", false),
    field(
        "endpoint",
        "Endpoint",
        "https://s3.example.com (blank for AWS)",
        false,
    ),
    field("access_key_id", "Access key ID", "AKIA…", true),
];

const WEBDAV_FIELDS: &[Field] = &[
    field("url", "URL", "https://dav.example.com/files", true),
    field("user", "User", "", false),
];

/// The form for a kind, in display order.
pub fn fields_for(kind: Kind) -> &'static [Field] {
    match kind {
        Kind::Ssh => SSH_FIELDS,
        Kind::S3 => S3_FIELDS,
        Kind::WebDav => WEBDAV_FIELDS,
    }
}

/// Longest connection name accepted.
pub const MAX_NAME: usize = 60;

/// What the user typed, before validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub kind: Kind,
    pub name: String,
    /// Values by field key.
    pub values: Vec<(String, String)>,
}

impl Draft {
    fn value(&self, key: &str) -> &str {
        self.values
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.trim())
            .unwrap_or("")
    }
}

/// Check a draft and produce the JSON that is stored. The error names the first
/// problem in words a form can show.
pub fn validate(draft: &Draft) -> Result<String, String> {
    let name = draft.name.trim();
    if name.is_empty() {
        return Err("Give the connection a name".to_string());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("The name is longer than {MAX_NAME} characters"));
    }
    let mut config = Map::new();
    for spec in fields_for(draft.kind) {
        let value = draft.value(spec.key);
        if value.is_empty() {
            if spec.required {
                return Err(format!("{} is required", spec.label));
            }
            continue;
        }
        config.insert(spec.key.to_string(), Value::String(value.to_string()));
    }
    match draft.kind {
        Kind::Ssh => {
            if let Some(Value::String(port)) = config.get("port") {
                match port.parse::<u16>() {
                    Ok(number) if number > 0 => {
                        config.insert("port".to_string(), json!(number));
                    }
                    _ => return Err("Port must be a number from 1 to 65535".to_string()),
                }
            }
            let host = draft.value("host");
            if host.contains(char::is_whitespace) || host.contains('/') {
                return Err("Host must be a plain host name or address".to_string());
            }
        }
        Kind::S3 => {
            if let Some(Value::String(endpoint)) = config.get("endpoint")
                && !is_http_url(endpoint)
            {
                return Err("Endpoint must start with http:// or https://".to_string());
            }
        }
        Kind::WebDav => {
            if !is_http_url(draft.value("url")) {
                return Err("URL must start with http:// or https://".to_string());
            }
        }
    }
    Ok(Value::Object(config).to_string())
}

fn is_http_url(text: &str) -> bool {
    ["http://", "https://"]
        .iter()
        .any(|scheme| text.len() > scheme.len() && text.starts_with(scheme))
}

/// One-line description of a stored connection for the list.
pub fn summary(kind: Kind, config: &str) -> String {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(config) else {
        return String::new();
    };
    let text = |key: &str| {
        map.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match kind {
        Kind::Ssh => {
            let port = map.get("port").and_then(Value::as_u64).unwrap_or(22);
            format!("{}@{}:{port}", text("user"), text("host"))
        }
        Kind::S3 => {
            let region = text("region");
            if region.is_empty() {
                format!("s3://{}", text("bucket"))
            } else {
                format!("s3://{} ({region})", text("bucket"))
            }
        }
        Kind::WebDav => text("url"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(kind: Kind, name: &str, values: &[(&str, &str)]) -> Draft {
        Draft {
            kind,
            name: name.to_string(),
            values: values
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn kinds_round_trip_through_their_stored_key() {
        for kind in Kind::ALL {
            assert_eq!(Kind::from_key(kind.key()), Some(kind));
            assert!(!kind.label().is_empty());
        }
        assert_eq!(Kind::from_key("ftp"), None);
    }

    #[test]
    fn stored_keys_are_the_documented_strings() {
        assert_eq!(Kind::Ssh.key(), "ssh");
        assert_eq!(Kind::S3.key(), "s3");
        assert_eq!(Kind::WebDav.key(), "webdav");
    }

    #[test]
    fn every_form_has_a_required_field_and_unique_keys() {
        for kind in Kind::ALL {
            let fields = fields_for(kind);
            assert!(fields.iter().any(|f| f.required), "{kind:?}");
            let mut keys: Vec<&str> = fields.iter().map(|f| f.key).collect();
            let before = keys.len();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), before, "{kind:?} repeats a field");
        }
    }

    #[test]
    fn a_complete_ssh_form_stores_only_the_filled_fields() {
        let json = validate(&draft(
            Kind::Ssh,
            "prod",
            &[
                ("host", "h.example.com"),
                ("user", "me"),
                ("port", " 2222 "),
            ],
        ))
        .unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["host"], "h.example.com");
        assert_eq!(value["port"], 2222, "stored as a number");
        assert!(value.get("key_path").is_none(), "blank fields are left out");
    }

    #[test]
    fn missing_required_fields_are_named() {
        let error = validate(&draft(Kind::Ssh, "x", &[("user", "me")])).unwrap_err();
        assert_eq!(error, "Host is required");
        assert_eq!(
            validate(&draft(Kind::S3, "x", &[("bucket", "b")])).unwrap_err(),
            "Access key ID is required"
        );
    }

    #[test]
    fn the_name_is_required_trimmed_and_bounded() {
        let ok = [("host", "h"), ("user", "u")];
        assert!(validate(&draft(Kind::Ssh, "   ", &ok)).is_err());
        assert!(validate(&draft(Kind::Ssh, &"n".repeat(MAX_NAME), &ok)).is_ok());
        assert!(validate(&draft(Kind::Ssh, &"n".repeat(MAX_NAME + 1), &ok)).is_err());
    }

    #[test]
    fn ports_must_be_real_port_numbers() {
        let with_port = |port: &str| {
            validate(&draft(
                Kind::Ssh,
                "x",
                &[("host", "h"), ("user", "u"), ("port", port)],
            ))
        };
        assert!(with_port("22").is_ok());
        assert!(with_port("65535").is_ok());
        for bad in ["0", "65536", "-1", "ssh", "22.5"] {
            assert!(with_port(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ssh_hosts_cannot_contain_spaces_or_slashes() {
        let host = |h: &str| validate(&draft(Kind::Ssh, "x", &[("host", h), ("user", "u")]));
        assert!(host("10.0.0.5").is_ok());
        assert!(host("a b").is_err());
        assert!(host("ssh://h").is_err());
    }

    #[test]
    fn urls_need_an_http_scheme() {
        let dav = |url: &str| validate(&draft(Kind::WebDav, "x", &[("url", url)]));
        assert!(dav("https://dav.example.com/files").is_ok());
        assert!(dav("http://localhost:8080").is_ok());
        for bad in ["dav.example.com", "ftp://x", "https://", ""] {
            assert!(dav(bad).is_err(), "{bad}");
        }
        let endpoint = |e: &str| {
            validate(&draft(
                Kind::S3,
                "x",
                &[("bucket", "b"), ("access_key_id", "k"), ("endpoint", e)],
            ))
        };
        assert!(endpoint("").is_ok(), "blank means AWS");
        assert!(endpoint("https://s3.example.com").is_ok());
        assert!(endpoint("s3.example.com").is_err());
    }

    #[test]
    fn summaries_read_back_what_was_stored() {
        let ssh = validate(&draft(
            Kind::Ssh,
            "x",
            &[("host", "h"), ("user", "me"), ("port", "2200")],
        ))
        .unwrap();
        assert_eq!(summary(Kind::Ssh, &ssh), "me@h:2200");
        let default_port =
            validate(&draft(Kind::Ssh, "x", &[("host", "h"), ("user", "me")])).unwrap();
        assert_eq!(summary(Kind::Ssh, &default_port), "me@h:22");
        let s3 = validate(&draft(
            Kind::S3,
            "x",
            &[
                ("bucket", "assets"),
                ("access_key_id", "k"),
                ("region", "eu-west-1"),
            ],
        ))
        .unwrap();
        assert_eq!(summary(Kind::S3, &s3), "s3://assets (eu-west-1)");
        let dav = validate(&draft(
            Kind::WebDav,
            "x",
            &[("url", "https://d.example.com")],
        ))
        .unwrap();
        assert_eq!(summary(Kind::WebDav, &dav), "https://d.example.com");
    }

    #[test]
    fn a_damaged_config_summarises_to_nothing_instead_of_failing() {
        assert_eq!(summary(Kind::Ssh, "not json"), "");
        assert_eq!(summary(Kind::S3, "[1,2]"), "");
    }

    #[test]
    fn secrets_have_no_field() {
        for kind in Kind::ALL {
            for field in fields_for(kind) {
                let key = field.key.to_lowercase();
                assert!(
                    !key.contains("password") && !key.contains("secret"),
                    "{key} would store a secret"
                );
            }
        }
    }
}
