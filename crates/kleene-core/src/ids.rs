//! Identifiers for conversations, runs, sessions, statements and calls.
//!
//! All are UUID v7 so they sort by creation time, which keeps trace tables
//! naturally ordered.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// A fresh, time-ordered identifier.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                s.parse().map(Self)
            }
        }
    };
}

id_type!(
    /// One invocation of the harness: a task attempt and everything under it.
    RunId
);
id_type!(
    /// One model session (root or sub-agent). Sessions form a tree via a parent id.
    SessionId
);
id_type!(
    /// A conversation: a sequence of runs where each task may refer to the
    /// ones before it. The TUI starts one when it opens and `/new` starts
    /// another; every run in it records its task and answer in the store's
    /// `conversations` table, so a later run can read them back.
    ConversationId
);
id_type!(
    /// One CallSQL statement submitted by a session.
    StatementId
);
id_type!(
    /// One model or tool call issued by an operator.
    CallId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_time_ordered() {
        let a = RunId::new();
        let b = RunId::new();
        assert!(a <= b);
    }

    #[test]
    fn ids_round_trip_through_json() {
        let id = SessionId::new();
        let s = serde_json::to_string(&id).unwrap();
        let back: SessionId = serde_json::from_str(&s).unwrap();
        assert_eq!(id, back);
    }
}
