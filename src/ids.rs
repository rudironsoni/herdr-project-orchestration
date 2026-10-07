//! Opaque ids. A prefix keeps a project id from being passed where a thread id
//! belongs. The value after the prefix is a ULID from the `ulid` crate.

use anyhow::{Result, bail};

macro_rules! id_type {
    ($name:ident, $prefix:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new() -> Self {
                Self(format!("{}_{}", $prefix, ulid::Ulid::generate()))
            }

            pub fn parse(text: &str) -> Result<Self> {
                let rest = text.strip_prefix(concat!($prefix, "_")).unwrap_or("");
                if rest.len() != 26 || !rest.chars().all(|c| c.is_ascii_alphanumeric()) {
                    bail!("`{text}` is not a {} id", $prefix);
                }
                Ok(Self(text.to_string()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(ProjectId, "prj");
id_type!(RepositoryId, "repo");
id_type!(WorkspaceId, "wks");
id_type!(ThreadId, "thr");
id_type!(SessionId, "ses");
id_type!(OperationId, "op");
id_type!(EventId, "evt");

/// The local machine. Remote environments use [`EnvironmentId`].
pub const ENV_LOCAL: &str = "env_local";

id_type!(EnvironmentId, "env");

impl EnvironmentId {
    pub fn local() -> Self {
        Self(ENV_LOCAL.to_string())
    }

    pub fn is_local(&self) -> bool {
        self.0 == ENV_LOCAL
    }

    pub fn from_db(text: &str) -> Result<Self> {
        if text == ENV_LOCAL {
            return Ok(Self::local());
        }
        Self::parse(text)
    }
}
