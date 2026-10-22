//! Saved remote connections: which kinds exist, what each one asks for, and how a
//! form is validated and stored.
//!
//! This is the part Settings needs to add, list and remove connections, plus the
//! step that turns a stored connection back into a [`RemoteSpec`] the backend can
//! open. Secrets (S3 secret keys, WebDAV passwords) are deliberately never part of
//! what is stored in the database: they live in the operating system's keychain
//! (see `secrets`). SFTP has no password at all, because it runs on the system
//! `ssh`, which authenticates with keys and the agent.

use crate::i18n::{tr, trf};
use serde_json::{Map, Value, json};
use vfs_core::{ProtocolScheme, RemoteSpec};

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
            Kind::Ssh => tr("SSH / SFTP"),
            Kind::S3 => tr("S3 / object storage"),
            Kind::WebDav => "WebDAV",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }

    /// The uri scheme a connection of this kind is browsed under.
    pub fn scheme(self) -> ProtocolScheme {
        match self {
            Kind::Ssh => ProtocolScheme::Sftp,
            Kind::S3 => ProtocolScheme::S3,
            Kind::WebDav => ProtocolScheme::WebDav,
        }
    }
}

/// One input in a connection form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub placeholder: &'static str,
    pub required: bool,
    /// Masked while typing, and kept out of the stored config (it goes to the keychain).
    pub secret: bool,
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
        secret: false,
    }
}

const fn secret_field(
    key: &'static str,
    label: &'static str,
    placeholder: &'static str,
    required: bool,
) -> Field {
    Field {
        secret: true,
        ..field(key, label, placeholder, required)
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
    secret_field("secret_access_key", "Secret access key", "", true),
];

const WEBDAV_FIELDS: &[Field] = &[
    field("url", "URL", "https://dav.example.com/files", true),
    field("user", "User", "", false),
    secret_field("password", "Password", "", false),
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
/// When `editing`, a blank secret is accepted: it means "keep the one already in the
/// keychain".
pub fn validate_with(draft: &Draft, editing: bool) -> Result<String, String> {
    let name = draft.name.trim();
    if name.is_empty() {
        return Err(tr("Give the connection a name").to_string());
    }
    if name.chars().count() > MAX_NAME {
        return Err(trf(
            "The name is longer than {} characters",
            &[&MAX_NAME.to_string()],
        ));
    }
    let mut config = Map::new();
    for spec in fields_for(draft.kind) {
        let value = draft.value(spec.key);
        if value.is_empty() {
            if spec.required && !(editing && spec.secret) {
                return Err(trf("{} is required", &[tr(spec.label)]));
            }
            continue;
        }
        if spec.secret {
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
                    _ => return Err(tr("Port must be a number from 1 to 65535").to_string()),
                }
            }
            let host = draft.value("host");
            if host.contains(char::is_whitespace) || host.contains('/') {
                return Err(tr("Host must be a plain host name or address").to_string());
            }
        }
        Kind::S3 => {
            if let Some(Value::String(endpoint)) = config.get("endpoint")
                && !is_http_url(endpoint)
            {
                return Err(tr("Endpoint must start with http:// or https://").to_string());
            }
        }
        Kind::WebDav => {
            if !is_http_url(draft.value("url")) {
                return Err(tr("URL must start with http:// or https://").to_string());
            }
        }
    }
    Ok(Value::Object(config).to_string())
}

/// The stored values of a connection as form text, so editing starts from what was
/// saved. Secrets are not in the config, so they are never among these.
pub fn stored_values(config: &str) -> Vec<(String, String)> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(config) else {
        return Vec::new();
    };
    map.into_iter()
        .filter_map(|(key, value)| match value {
            Value::String(text) => Some((key, text)),
            Value::Number(number) => Some((key, number.to_string())),
            _ => None,
        })
        .collect()
}

/// The one secret a draft carries (S3 secret key, WebDAV password), if filled in.
/// Kept apart from [`validate`]'s output so it can never end up in the database.
pub fn secret_of(draft: &Draft) -> Option<String> {
    let spec = fields_for(draft.kind).iter().find(|f| f.secret)?;
    let value = draft.value(spec.key);
    (!value.is_empty()).then(|| value.to_string())
}

/// Turn a stored config plus its keychain secret into something the backend opens.
pub fn to_spec(kind: Kind, config: &str, secret: Option<String>) -> Result<RemoteSpec, String> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(config) else {
        return Err(tr("The saved connection is damaged").to_string());
    };
    let text = |key: &str| {
        map.get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let need = |key: &str, label: &'static str| {
        text(key).ok_or_else(|| trf("{} is required", &[tr(label)]))
    };
    Ok(match kind {
        Kind::Ssh => RemoteSpec::Sftp {
            host: need("host", "Host")?,
            port: map
                .get("port")
                .and_then(Value::as_u64)
                .and_then(|port| u16::try_from(port).ok()),
            user: need("user", "User")?,
            key_path: text("key_path"),
        },
        Kind::S3 => RemoteSpec::S3 {
            bucket: need("bucket", "Bucket")?,
            region: text("region"),
            endpoint: text("endpoint"),
            access_key_id: need("access_key_id", "Access key ID")?,
            secret_access_key: secret.ok_or_else(|| {
                tr("The secret access key is missing from the keychain").to_string()
            })?,
        },
        Kind::WebDav => RemoteSpec::WebDav {
            url: need("url", "URL")?,
            user: text("user"),
            password: secret,
        },
    })
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

    fn validate(draft: &Draft) -> Result<String, String> {
        validate_with(draft, false)
    }

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
                &[
                    ("bucket", "b"),
                    ("access_key_id", "k"),
                    ("secret_access_key", "s"),
                    ("endpoint", e),
                ],
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
                ("secret_access_key", "s"),
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
    fn secret_fields_never_reach_the_stored_config() {
        let draft = draft(
            Kind::S3,
            "x",
            &[
                ("bucket", "b"),
                ("access_key_id", "AKID"),
                ("secret_access_key", "topsecret"),
            ],
        );
        let json = validate(&draft).unwrap();
        assert!(!json.contains("topsecret"), "{json}");
        assert!(!json.contains("secret_access_key"), "{json}");
        assert_eq!(secret_of(&draft).as_deref(), Some("topsecret"));
    }

    #[test]
    fn every_kind_has_at_most_one_secret_and_only_masks_it() {
        for kind in Kind::ALL {
            let secrets: Vec<_> = fields_for(kind).iter().filter(|f| f.secret).collect();
            assert!(
                secrets.len() <= 1,
                "{kind:?} needs a keychain slot per secret"
            );
        }
        // SFTP authenticates through the system ssh: nothing to type or store.
        assert!(fields_for(Kind::Ssh).iter().all(|f| !f.secret));
    }

    #[test]
    fn the_s3_secret_is_required_and_the_webdav_password_is_not() {
        let missing = validate(&draft(
            Kind::S3,
            "x",
            &[("bucket", "b"), ("access_key_id", "k")],
        ))
        .unwrap_err();
        assert_eq!(missing, "Secret access key is required");
        let dav = draft(Kind::WebDav, "x", &[("url", "https://d.example.com")]);
        assert!(validate(&dav).is_ok());
        assert_eq!(secret_of(&dav), None);
    }

    #[test]
    fn a_stored_ssh_connection_becomes_an_sftp_spec() {
        let json = validate(&draft(
            Kind::Ssh,
            "x",
            &[
                ("host", "h.example"),
                ("user", "me"),
                ("port", "2222"),
                ("key_path", "~/.ssh/id"),
            ],
        ))
        .unwrap();
        assert_eq!(
            to_spec(Kind::Ssh, &json, None).unwrap(),
            RemoteSpec::Sftp {
                host: "h.example".into(),
                port: Some(2222),
                user: "me".into(),
                key_path: Some("~/.ssh/id".into()),
            }
        );
    }

    #[test]
    fn a_stored_s3_connection_needs_its_secret_from_the_keychain() {
        let json = validate(&draft(
            Kind::S3,
            "x",
            &[
                ("bucket", "b"),
                ("access_key_id", "AKID"),
                ("secret_access_key", "s"),
            ],
        ))
        .unwrap();
        assert!(to_spec(Kind::S3, &json, None).is_err());
        let spec = to_spec(Kind::S3, &json, Some("from-keychain".into())).unwrap();
        assert_eq!(
            spec,
            RemoteSpec::S3 {
                bucket: "b".into(),
                region: None,
                endpoint: None,
                access_key_id: "AKID".into(),
                secret_access_key: "from-keychain".into(),
            }
        );
    }

    #[test]
    fn a_stored_webdav_connection_works_without_credentials() {
        let json = validate(&draft(
            Kind::WebDav,
            "x",
            &[("url", "https://d.example.com")],
        ))
        .unwrap();
        assert_eq!(
            to_spec(Kind::WebDav, &json, None).unwrap(),
            RemoteSpec::WebDav {
                url: "https://d.example.com".into(),
                user: None,
                password: None,
            }
        );
    }

    #[test]
    fn a_damaged_stored_config_is_an_error_not_a_panic() {
        for kind in Kind::ALL {
            assert!(to_spec(kind, "not json", None).is_err());
            assert!(to_spec(kind, "{}", Some("s".into())).is_err());
        }
    }

    #[test]
    fn editing_accepts_a_blank_secret_but_creating_does_not() {
        let blank = draft(
            Kind::S3,
            "x",
            &[
                ("bucket", "b"),
                ("access_key_id", "k"),
                ("secret_access_key", ""),
            ],
        );
        assert!(validate(&blank).is_err());
        assert!(validate_with(&blank, true).is_ok());
        // Other required fields are still enforced while editing.
        let no_bucket = draft(Kind::S3, "x", &[("access_key_id", "k")]);
        assert!(validate_with(&no_bucket, true).is_err());
    }

    #[test]
    fn stored_values_round_trip_into_form_text() {
        let json = validate(&draft(
            Kind::Ssh,
            "x",
            &[("host", "h"), ("user", "me"), ("port", "2222")],
        ))
        .unwrap();
        let values = stored_values(&json);
        let get = |key: &str| {
            values
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("host"), Some("h"));
        assert_eq!(get("port"), Some("2222"), "numbers come back as text");
        assert_eq!(get("key_path"), None);
        assert!(stored_values("not json").is_empty());
    }

    #[test]
    fn kinds_map_to_distinct_schemes() {
        assert_eq!(Kind::Ssh.scheme(), ProtocolScheme::Sftp);
        assert_eq!(Kind::S3.scheme(), ProtocolScheme::S3);
        assert_eq!(Kind::WebDav.scheme(), ProtocolScheme::WebDav);
    }
}
