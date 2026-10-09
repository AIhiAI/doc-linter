//! Direct unit tests for the SCIP-symbol path helpers used by the
//! [[entity-doc-graph]] ingest. Exercises the prefix / decoration /
//! descends-from contract without spinning up a full graph round-trip
//! so a regression in the string-handling fires on the offending
//! helper, not at the integration layer.

use super::{
    strip_scip_descriptor_decoration, strip_scip_package_prefix, symbol_descends_from,
    symbol_descriptor_prefix,
};

#[test]
fn strip_package_prefix_skips_four_whitespace_tokens() {
    assert_eq!(
        strip_scip_package_prefix("rust-analyzer cargo pricing 0.1.0 model/PricingRule#"),
        Some("model/PricingRule#")
    );
}

#[test]
fn strip_package_prefix_returns_none_on_short_symbols() {
    assert_eq!(
        strip_scip_package_prefix("rust-analyzer cargo pricing model/PricingRule#"),
        None
    );
    assert_eq!(strip_scip_package_prefix(""), None);
    assert_eq!(strip_scip_package_prefix("only-one-token"), None);
}

#[test]
fn strip_decoration_removes_method_suffix() {
    assert_eq!(strip_scip_descriptor_decoration("validate()."), "validate");
    assert_eq!(strip_scip_descriptor_decoration("validate()"), "validate");
    assert_eq!(strip_scip_descriptor_decoration("validate."), "validate");
}

#[test]
fn strip_decoration_removes_type_marker() {
    assert_eq!(
        strip_scip_descriptor_decoration("PricingRule#"),
        "PricingRule"
    );
    assert_eq!(
        strip_scip_descriptor_decoration("PricingRule#."),
        "PricingRule"
    );
}

#[test]
fn strip_decoration_keeps_brackets() {
    assert_eq!(strip_scip_descriptor_decoration("Foo[Bar]"), "Foo[Bar]");
    assert_eq!(strip_scip_descriptor_decoration("Foo[Bar]."), "Foo[Bar]");
}

#[test]
fn strip_decoration_no_op_for_undecorated() {
    assert_eq!(
        strip_scip_descriptor_decoration("PricingRule"),
        "PricingRule"
    );
    assert_eq!(strip_scip_descriptor_decoration(""), "");
}

#[test]
fn descriptor_prefix_keeps_segments_strips_decoration() {
    assert_eq!(
        symbol_descriptor_prefix("rust-analyzer cargo p 0.1.0 model/PricingRule/validate().")
            .as_deref(),
        Some("model/PricingRule/validate")
    );
    assert_eq!(
        symbol_descriptor_prefix("rust-analyzer cargo p 0.1.0 outlets/handlers/get_outlet().")
            .as_deref(),
        Some("outlets/handlers/get_outlet")
    );
}

#[test]
fn descriptor_prefix_strips_decoration_per_segment() {
    assert_eq!(
        symbol_descriptor_prefix("rust-analyzer cargo p 0.1.0 model/PricingRule#").as_deref(),
        Some("model/PricingRule")
    );
}

#[test]
fn descriptor_prefix_returns_none_on_malformed_inputs() {
    assert_eq!(symbol_descriptor_prefix("not-a-scip-symbol"), None);
    assert_eq!(
        symbol_descriptor_prefix("rust-analyzer cargo pricing 0.1.0 "),
        None
    );
}

#[test]
fn descends_from_requires_separator_boundary() {
    assert!(symbol_descends_from(
        "rust-analyzer cargo p 0.1.0 model/PricingRule/validate().",
        "model/PricingRule"
    ));
    assert!(!symbol_descends_from(
        "rust-analyzer cargo p 0.1.0 model/PricingRuleHelper/work().",
        "model/PricingRule"
    ));
}

#[test]
fn descends_from_self_is_false() {
    assert!(!symbol_descends_from(
        "rust-analyzer cargo p 0.1.0 model/PricingRule#",
        "model/PricingRule"
    ));
}

#[test]
fn descends_from_malformed_inputs_return_false() {
    assert!(!symbol_descends_from("garbage", "model/PricingRule"));
}
