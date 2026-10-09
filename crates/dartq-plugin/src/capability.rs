//! Capability vocabulary and negotiation for the dartiq plugin protocol.
//!
//! The negotiated set is always the **intersection** of the core's supported
//! capabilities and the plugin's advertised ones. Unknown capability names on
//! either side are ignored — they mean "not enabled", never an error — so a
//! newer plugin can advertise capabilities an older core does not know about
//! (PRD §14.1: the wire protocol layer carries the version adaptation).

use serde_json::Value;

/// A capability a plugin can expose. The wire names are lowercase strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Capability {
    /// The plugin contributes lint rules.
    Lint,
    /// The plugin contributes fixes for diagnostics.
    Fix,
    /// The plugin contributes assists.
    Assist,
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.wire_name())
    }
}

impl Capability {
    /// Every capability defined by protocol v1.
    pub const ALL: [Capability; 3] = [Capability::Lint, Capability::Fix, Capability::Assist];

    /// Parses a wire name; unknown names yield `None` (never an error).
    #[must_use]
    pub fn from_wire_name(name: &str) -> Option<Self> {
        match name {
            "lint" => Some(Capability::Lint),
            "fix" => Some(Capability::Fix),
            "assist" => Some(Capability::Assist),
            _ => None,
        }
    }

    /// The lowercase name used on the wire.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Capability::Lint => "lint",
            Capability::Fix => "fix",
            Capability::Assist => "assist",
        }
    }
}

/// A set of known capabilities, negotiated as the intersection of both sides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapabilitySet(u8);

impl CapabilitySet {
    /// The empty set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Every capability of protocol v1.
    #[must_use]
    pub const fn all() -> Self {
        Self(0b111)
    }

    /// Enables `cap`.
    pub fn insert(&mut self, cap: Capability) {
        self.0 |= 1 << (cap as u8);
    }

    /// Adds the capabilities named in a JSON value (the `capabilities` array
    /// of a handshake frame). Unknown names and non-string entries are
    /// ignored: an unknown capability is "not enabled", never an error.
    pub fn insert_wire_names(&mut self, value: Option<&Value>) {
        let Some(Value::Array(items)) = value else {
            return;
        };
        for item in items {
            if let Value::String(name) = item
                && let Some(cap) = Capability::from_wire_name(name)
            {
                self.insert(cap);
            }
        }
    }

    /// Whether `cap` is enabled.
    #[must_use]
    pub const fn contains(self, cap: Capability) -> bool {
        self.0 & (1 << (cap as u8)) != 0
    }

    /// The negotiated set: capabilities present in both operands.
    #[must_use]
    pub const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Whether no capability is enabled.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The enabled capabilities as wire names, in protocol order.
    #[must_use]
    pub fn names(self) -> Vec<&'static str> {
        Capability::ALL
            .iter()
            .filter(|cap| self.contains(**cap))
            .map(|cap| cap.wire_name())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wire_name_round_trip() {
        for cap in Capability::ALL {
            assert_eq!(Capability::from_wire_name(cap.wire_name()), Some(cap));
        }
        assert_eq!(Capability::from_wire_name("completions"), None);
    }

    #[test]
    fn intersection_keeps_only_shared_capabilities() {
        let core = CapabilitySet::all();
        let mut plugin = CapabilitySet::empty();
        plugin.insert(Capability::Lint);
        let effective = core.intersect(plugin);
        assert!(effective.contains(Capability::Lint));
        assert!(!effective.contains(Capability::Fix));
        assert_eq!(effective.names(), vec!["lint"]);
    }

    #[test]
    fn unknown_wire_names_are_ignored_not_errors() {
        let mut caps = CapabilitySet::empty();
        caps.insert_wire_names(Some(&json!(["lint", "completions", 42, null, "fix"])));
        assert!(caps.contains(Capability::Lint));
        assert!(caps.contains(Capability::Fix));
        assert!(!caps.contains(Capability::Assist));

        // A missing or malformed `capabilities` field enables nothing.
        let mut none = CapabilitySet::empty();
        none.insert_wire_names(None);
        none.insert_wire_names(Some(&json!("lint")));
        assert!(none.is_empty());
    }
}
