//! Share immutable source-sized tool arguments between request snapshots.

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

/// A tool call's verbatim JSON arguments, shared rather than copied.
///
/// A `write` call contains the complete generated file. The active request,
/// retained wire prefix, per-round request and prompt-caching copy all need
/// those same bytes; cloning this handle keeps one source allocation alive.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolArguments(Arc<String>);

impl ToolArguments {
    /// The original JSON text, including whitespace and escape spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Buffer capacity and shared-allocation metadata kept alive by a handle.
    /// A streamed String can have substantial spare capacity; sharing it must
    /// charge that capacity to the retained-prefix budget too.
    pub(crate) fn retained_bytes(&self) -> usize {
        // The Arc holds the String metadata plus strong and weak counters.
        self.0
            .capacity()
            .saturating_add(std::mem::size_of::<String>())
            .saturating_add(std::mem::size_of::<[usize; 2]>())
    }
}

impl Deref for ToolArguments {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Debug for ToolArguments {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for ToolArguments {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<String> for ToolArguments {
    fn from(arguments: String) -> Self {
        Self(Arc::new(arguments))
    }
}

impl From<&str> for ToolArguments {
    fn from(arguments: &str) -> Self {
        arguments.to_string().into()
    }
}

impl PartialEq<str> for ToolArguments {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for ToolArguments {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl serde::Serialize for ToolArguments {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taking_an_owned_string_keeps_its_allocation() {
        let source = String::from(r#" { "content": "猫\n" } "#);
        let bytes = source.as_ptr();
        let arguments = ToolArguments::from(source);
        assert_eq!(arguments.as_ptr(), bytes);
        assert_eq!(arguments.as_str(), r#" { "content": "猫\n" } "#);
        assert_eq!(arguments, arguments.clone());
    }
}
