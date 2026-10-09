//! Typed newtype wrappers for the four id-shaped strings that flow
//! through the [[entity-doc-graph]] linter:
//!
//!   - [`DocId`] — a doc node's id (frontmatter `id:` / filename stem).
//!   - [`EntityId`] — an ontology entity's id (`value_id:` on entity
//!     docs).
//!   - [`FunctionSymbol`] — a SCIP-format function symbol (the
//!     `rust-analyzer cargo crate 0.1.0 mod/foo().` shape).
//!
//! `String` flowed through every store signature before #54 —
//! `outbound(&db, &entity_id)` compiled even when the caller meant a
//! `doc_id`, returning `[]` silently. The newtypes make that swap a
//! compile error.
//!
//! All three are `#[serde(transparent)]`, so JSON output is byte-
//! identical to the pre-#54 shape. Wrap a `String` (or `&str`) with
//! `DocId::from(s)`; unwrap with `.0` (the inner `String` is `pub`)
//! or `.as_str()` (via the `AsRef<str>` impl).

use serde::{Deserialize, Serialize};
use std::fmt;

/// A doc-node id — the value of `id:` in a doc's frontmatter, which
/// must equal the filename stem. Used as the primary key in the
/// [[entity-doc-graph]] Doc table.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocId(pub String);

/// An ontology entity's id — the value of `value_id:` in an entity
/// doc's frontmatter. Primary key of the [[entity-doc-graph]] Entity
/// table and the right-hand side of `COVERS` / `MENTIONS` /
/// `FUNCTION_MENTIONS` edges.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub String);

/// A SCIP-format function symbol — the `rust-analyzer cargo
/// <crate> <version> <descriptor-path>().` shape that
/// `rust-analyzer scip` emits. Primary key of the
/// [[entity-doc-graph]] Function table.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FunctionSymbol(pub String);

macro_rules! impl_id_traits {
    ($t:ty) => {
        impl $t {
            /// Borrow the inner string for read-only access (formatting,
            /// substring search, the store parameter construction).
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume the wrapper and return the owned inner `String` —
            /// used at the store boundary where the underlying API takes
            /// `Value::String(String)`.
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl From<String> for $t {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $t {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl AsRef<str> for $t {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

impl_id_traits!(DocId);
impl_id_traits!(EntityId);
impl_id_traits!(FunctionSymbol);

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;

    #[test]
    fn ids_serialize_transparently() {
        let doc = DocId::from("roadmap-43");
        let entity = EntityId::from("pricing-rule");
        let symbol = FunctionSymbol::from("rust-analyzer cargo demo 0.1.0 foo().");

        // #[serde(transparent)] — the wrapper does not add an outer
        // object or array; the JSON is the bare string the wrapper
        // holds.
        assert_eq!(serde_json::to_string(&doc).unwrap(), "\"roadmap-43\"");
        assert_eq!(serde_json::to_string(&entity).unwrap(), "\"pricing-rule\"");
        assert_eq!(
            serde_json::to_string(&symbol).unwrap(),
            "\"rust-analyzer cargo demo 0.1.0 foo().\""
        );
    }

    #[test]
    fn ids_roundtrip_through_json() {
        let original = EntityId::from("pricing-rule");
        let s = serde_json::to_string(&original).unwrap();
        let parsed: EntityId = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn ids_display_and_as_ref_match_inner() {
        let id = DocId::from("foo");
        assert_eq!(id.to_string(), "foo");
        assert_eq!(id.as_str(), "foo");
        let inner: &str = id.as_ref();
        assert_eq!(inner, "foo");
    }

    #[test]
    fn different_id_types_are_not_interchangeable() {
        // This test is a compile-time guarantee, not a runtime one —
        // having it here documents the invariant. Uncommenting the
        // body fails to compile:
        //
        //   let doc: DocId = EntityId::from("x");  // mismatched types
        //
        // Same wrapper, different newtype — the compiler refuses.
        let _doc = DocId::from("x");
        let _entity = EntityId::from("x");
    }

    // Property tests (#56) — algebraic invariants on the newtypes.
    // proptest generates ~256 random strings per property and shrinks
    // failing cases down to a minimal counterexample.
    proptest::proptest! {
        /// `DocId::from(s).as_str() == s` for every string `s`.
        /// The wrapper is a transparent newtype around `String`; the
        /// inner bytes are never mutated by `From` / `as_str`. Holds
        /// for any UTF-8 (`String` only allows valid UTF-8 by type).
        #[test]
        fn doc_id_from_str_is_identity(s in ".*") {
            let id = DocId::from(s.as_str());
            proptest::prop_assert_eq!(id.as_str(), s.as_str());
        }

        /// Same identity invariant for [`EntityId`].
        #[test]
        fn entity_id_from_str_is_identity(s in ".*") {
            let id = EntityId::from(s.as_str());
            proptest::prop_assert_eq!(id.as_str(), s.as_str());
        }

        /// Same identity invariant for [`FunctionSymbol`].
        #[test]
        fn function_symbol_from_str_is_identity(s in ".*") {
            let sym = FunctionSymbol::from(s.as_str());
            proptest::prop_assert_eq!(sym.as_str(), s.as_str());
        }

        /// JSON round-trip is byte-identical: serialize a wrapper,
        /// deserialize, the wrapped string is unchanged. Asserts
        /// `#[serde(transparent)]` actually delivers what the doc
        /// comment claims.
        #[test]
        fn doc_id_json_roundtrip(s in ".*") {
            let original = DocId::from(s.as_str());
            let json = serde_json::to_string(&original).unwrap();
            let parsed: DocId = serde_json::from_str(&json).unwrap();
            proptest::prop_assert_eq!(parsed, original);
        }
    }
}
