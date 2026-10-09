//! Round 3B: SCIP (Sourcegraph Code Intelligence Protocol) ingest.
//!
//! Parses a SCIP index file (produced by `rust-analyzer scip` or another
//! SCIP-emitting tool) and yields a flat list of `FunctionFact`s ready to
//! be folded into the SQLite graph by `store::ingest_scip`. The
//! parser walks every "kind" worth pinning to a Doc/Entity (functions,
//! methods, structs, enums, traits, modules, type aliases). Type-kinds
//! land in the dedicated `Type` node table; only true callables stay in
//! `Function`. Doc-comments attached to definitions are the load-bearing
//! payload: that's what the entity-mention scanner runs against.
//!
//! ## Workflow
//!
//! 1. The user (or CI) runs `rust-analyzer scip . --output .doc-lint/code.scip`.
//!    `doc-linter scip-index` wraps that invocation when `rust-analyzer`
//!    is on `PATH`.
//! 2. `cmd_check` calls `parse_scip(<root>/.doc-lint/code.scip, root)` if
//!    the file exists, then dispatches to `ingest_scip` via the cache
//!    layer in `scip_ingest_cache` — unchanged SCIP files skip the parse
//!    + insert phases entirely and only rederive cross-bucket edges.
//! 3. On a miss (or `--rebuild`), `ingest_scip` partitions facts by kind,
//!    bulk-inserts `Function` and `Type` nodes into their respective
//!    tables, and emits `FUNCTION_DEFINED_IN` / `FUNCTION_MENTIONS` /
//!    `TYPE_DEFINED_IN` / `TYPE_MENTIONS` edges into the existing schema.
//!
//! ## Tested against
//!
//! - `scip` crate v0.7.x (latest as of 2026-04-30).
//! - `rust-analyzer` SCIP output v0.6+ (the `scip` subcommand is
//!   present and stable in 2024-25 nightly builds).
//!
//! If `rust-analyzer scip --help` ever changes its argument shape, only
//! the wrapper in `main.rs::cmd_scip_index` needs updating; the parser
//! here only depends on the SCIP wire format, which is governed by
//! sourcegraph/scip and versioned independently.

use anyhow::{bail, Context, Result};
use protobuf::Message;
use scip::types::symbol_information::Kind as ScipKind;
use scip::types::Index;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Coarse-grained classification of a SCIP `SymbolInformation.kind`.
/// Mirrors the SCIP enum variants we care about and lumps everything
/// else into `Other`. Stored on `FunctionFact` so downstream consumers
/// (query subcommands, future sub-graphs) can filter by the conceptual
/// node-kind without having to deserialise the SCIP enum integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FunctionKind {
    Function,
    Method,
    Struct,
    Enum,
    Trait,
    Module,
    TypeAlias,
    Other,
}

impl FunctionKind {
    /// Stable string label, used by inline unit tests to assert
    /// classifier output without leaking `Debug` formatting into the
    /// asserts.
    #[cfg(test)]
    pub fn as_str(self) -> &'static str {
        match self {
            FunctionKind::Function => "function",
            FunctionKind::Method => "method",
            FunctionKind::Struct => "struct",
            FunctionKind::Enum => "enum",
            FunctionKind::Trait => "trait",
            FunctionKind::Module => "module",
            FunctionKind::TypeAlias => "type-alias",
            FunctionKind::Other => "other",
        }
    }

    fn from_scip(kind: ScipKind) -> FunctionKind {
        match kind {
            ScipKind::Function => FunctionKind::Function,
            ScipKind::Method
            | ScipKind::AbstractMethod
            | ScipKind::TraitMethod
            | ScipKind::StaticMethod
            | ScipKind::Constructor
            | ScipKind::SingletonMethod
            | ScipKind::PureVirtualMethod
            | ScipKind::ProtocolMethod => FunctionKind::Method,
            ScipKind::Struct | ScipKind::Class => FunctionKind::Struct,
            ScipKind::Enum => FunctionKind::Enum,
            ScipKind::Trait | ScipKind::Interface => FunctionKind::Trait,
            ScipKind::Module | ScipKind::Namespace | ScipKind::Package => FunctionKind::Module,
            ScipKind::TypeAlias | ScipKind::Type => FunctionKind::TypeAlias,
            _ => FunctionKind::Other,
        }
    }
}

/// Fallback kind inference from a SCIP symbol's descriptor suffix.
///
/// Some indexers (notably `scip-python` 0.6.x) emit a valid SCIP file but
/// leave `SymbolInformation.kind` at the default `UnspecifiedKind` for
/// every symbol. Without this fallback, every Python repo would silently
/// ingest zero functions. Returns `None` when the descriptor maps to a
/// kind we already classify as `Other` (Term, Local, Parameter, ...), so
/// the caller preserves the existing "drop noisy non-definitional
/// symbols" behaviour.
fn infer_kind_from_descriptor(symbol: &str) -> Option<FunctionKind> {
    use scip::types::descriptor::Suffix;

    let parsed = scip::symbol::parse_symbol(symbol).ok()?;
    let last = parsed.descriptors.last()?;
    let suffix = last.suffix.enum_value().ok()?;

    match suffix {
        Suffix::Method => {
            // `Type#name().` is a method on a type; otherwise a free function.
            let on_type = parsed
                .descriptors
                .iter()
                .rev()
                .skip(1)
                .any(|d| matches!(d.suffix.enum_value(), Ok(Suffix::Type)));
            Some(if on_type {
                FunctionKind::Method
            } else {
                FunctionKind::Function
            })
        }
        Suffix::Type => Some(FunctionKind::Struct),
        Suffix::Namespace | Suffix::Package => Some(FunctionKind::Module),
        _ => None,
    }
}

/// One ingested code-symbol fact ready for graph insertion.
#[derive(Debug, Clone)]
pub struct FunctionFact {
    /// Full SCIP symbol string. Globally unique within the index;
    /// used as the Function table primary key.
    pub symbol: String,
    /// Crate name extracted from either the SCIP package descriptor or
    /// the file path. Empty when neither yields a result (rare; would
    /// mean a sample/fixture path outside `crates/`).
    pub crate_name: String,
    /// File path relative to the repo root. The SCIP indexer already
    /// emits relative paths in `Document.relative_path`.
    pub file: String,
    /// 1-based first line of the definition. SCIP ranges are 0-based;
    /// we add 1 so the value matches what an editor's gutter shows.
    pub line: u32,
    /// Joined documentation lines (Markdown). Empty when the symbol has
    /// no doc-comment attached. This is the haystack for entity-mention
    /// scanning.
    pub doc_comment: String,
    /// Coarse kind classification — see `FunctionKind`. Read only by
    /// the inline unit tests below; production code-paths don't
    /// branch on kind yet but the field is captured at parse time so
    /// tests can assert the classifier and #51 (typed enums) can
    /// promote it.
    #[allow(dead_code)] // test-only read; field is built during ingest for future use
    pub kind: FunctionKind,
    /// Roadmap issue #30 (v0.3.0): source language derived from the
    /// file extension. One of `"rust"`, `"python"`, `"typescript"`,
    /// `"tsx"`, `"javascript"`, `"unknown"`. Persisted on the
    /// Function row so per-language analyses can subset without a
    /// path-suffix LIKE scan.
    pub language: String,
    /// Roadmap issue #22 (v0.3.0): one-line signature from SCIP's
    /// `signature_documentation` field. Empty when the indexer
    /// didn't populate it — scip-python and older rust-analyzer
    /// builds omit it.
    pub signature: String,
    /// Roadmap issue #22 (v0.3.0): first ~15 lines of the
    /// function body, capped at 500 bytes. Read from the source
    /// file at `line` during ingest. Empty when the file can't be
    /// read (deleted, permission error) — agents fall back to a
    /// Read tool call in that case.
    pub body_excerpt: String,
    /// Roadmap issue #26 (v0.3.0): git author-timestamp (ISO-8601)
    /// of the most recent commit that touched the source file.
    /// Empty when the repo isn't a git worktree or the file isn't
    /// tracked. Lets agents detect stale docs by joining
    /// Doc.updated against Function.last_touched.
    pub last_touched: String,
    /// A compiler-generated Java member (Lombok accessor, record
    /// accessor, enum `values()`): callable, so it stays a call target,
    /// but it has no source of its own and is left out of every
    /// documentation / coverage measure.
    pub generated: bool,
}

/// One ingested file-to-file import edge ready for the the store
/// `IMPORTS` rel table. Derived from SCIP occurrences whose
/// `symbol_roles` carries the Import bit (= 2). See roadmap
/// issue #16 — the file-level import graph is cleaner than the
/// pre-#16 FUNCTION_MENTIONS co-occurrence inference because
/// SCIP marks import sites explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImportFact {
    /// File doing the importing (the SCIP Document the
    /// occurrence lives in). Repo-relative.
    pub importer_file: String,
    /// File whose symbol is being imported. Derived from the
    /// SCIP symbol's descriptor path. Repo-relative; empty when
    /// the symbol is a local / cross-package reference that
    /// doesn't resolve to a file (those facts get dropped at
    /// edge-emit time).
    pub imported_file: String,
}

/// One ingested function-to-function call edge ready for the the store
/// `CALLS` rel table. Source-side (`caller_symbol`) is always a
/// function-kind symbol defined in the SCIP index; callee is any
/// symbol whose descriptor ends with `().` (the SCIP shape for
/// function / method references — see roadmap issue #9). Cross-file
/// callees are emitted with the caller's source location so chain
/// queries can attribute hops to specific call sites.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallFact {
    /// SCIP symbol of the function whose body contains the call site.
    pub caller_symbol: String,
    /// SCIP symbol being referenced. Not required to be defined in the
    /// same SCIP index — external callees just won't get a the store edge
    /// (the MATCH on the Function table fails silently).
    pub callee_symbol: String,
    /// Source file (repo-relative) where the call site lives — i.e.
    /// the caller's defining file.
    pub source_file: String,
    /// 1-based line of the call site. Matches the `line` convention
    /// used by `FunctionFact` so editor jumps line up.
    pub source_line: u32,
    /// The call names an interface / abstract / overridden method and
    /// this edge goes to one of its implementations (#268): dynamic
    /// dispatch the call site doesn't spell out.
    pub dispatch: bool,
}

/// Roadmap issue #32 v6 (v0.4.0): one Type → Type "implements" edge
/// derived from a SCIP `SymbolInformation.relationships` entry with
/// `is_implementation: true`. Emitted per `(from_type, to_type)`
/// pair; the parser keeps both sides verbatim — the edge insert `...
/// CREATE` drops edges whose endpoints aren't both in the `Type`
/// table (cross-crate / stdlib trait impls), matching the
/// established `CALLS` / `USES_TYPE` convention.
///
/// SCIP semantics: `is_implementation` covers both Rust `impl Trait
/// for Type` ("class satisfies interface") and trait/class
/// inheritance ("specialises another type"). v8 (issue #32 v8)
/// splits the two by FROM/TO kind shape — see [`ExtendsFact`].
/// Whatever doesn't classify as EXTENDS lands here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImplementsFact {
    /// SCIP symbol of the implementing type (the FROM side).
    pub from_type: String,
    /// SCIP symbol of the implemented trait / interface / parent
    /// class (the TO side).
    pub to_type: String,
}

/// Roadmap issue #32 v8 (v0.4.0): one Type → Type "extends" edge —
/// the inheritance counterpart to [`ImplementsFact`]. SCIP doesn't
/// ship a dedicated `is_extends` flag, so this fact is derived from
/// the same `is_implementation: true` relationships and split out
/// when the FROM/TO kind pair indicates inheritance rather than
/// interface satisfaction.
///
/// Classification rule (cross-language):
/// - `(Trait, Trait)` → EXTENDS — Rust `trait Foo: Bar`, TypeScript
///   `interface Foo extends Bar`, Java `interface Foo extends Bar`.
/// - Everything else with `is_implementation: true` →
///   [`ImplementsFact`].
///
/// The `(Class, Class)` case in single-inheritance languages
/// (Java / Python) maps to SCIP's `Class` kind which we treat as
/// `Struct`. Without a language column on the FROM symbol we'd risk
/// classifying every Rust `struct ... impl Trait for ...` as EXTENDS
/// when the underlying SCIP shape isn't actually inheritance. v8
/// stays conservative: `(Trait, Trait)` only, defer broader rules
/// until a v0.5.0 language-aware classifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExtendsFact {
    /// SCIP symbol of the descendant type — the trait doing the
    /// extending (the FROM side).
    pub from_type: String,
    /// SCIP symbol of the parent trait being extended (the TO side).
    pub to_type: String,
}

/// A field / property definition: a symbol whose last descriptor is a
/// term (`.`) directly under a type (`#`) — `Product#isFreeProduct.`.
/// The same shape across scip-dotnet (properties and fields),
/// scip-typescript, scip_dart, rust-analyzer and scip-java.
#[derive(Debug, Clone)]
pub struct FieldFact {
    pub symbol: String,
    /// The bare field name (`isFreeProduct`).
    pub name: String,
    /// SCIP symbol of the owning type (`…/Product#`).
    pub owner: String,
    pub file: String,
    pub line: u32,
    pub doc_comment: String,
    pub language: String,
}

/// A function that reads or writes a field, at its first reference in
/// that function. Deduped per (function, field).
#[derive(Debug, Clone)]
pub struct ReferenceFact {
    pub function_symbol: String,
    pub field_symbol: String,
    pub source_file: String,
    pub source_line: u32,
}

/// `(owner, name)` when `symbol` is a field / property (`Type#name.`),
/// `None` otherwise (methods, types, parameters, locals, module-level
/// terms).
pub fn field_parts(symbol: &str) -> Option<(String, String)> {
    use scip::types::descriptor::Suffix;
    if symbol.starts_with("local ") {
        return None;
    }
    let parsed = scip::symbol::parse_symbol(symbol).ok()?;
    let mut descriptors = parsed.descriptors.iter().rev();
    let last = descriptors.next()?;
    let parent = descriptors.next()?;
    if last.suffix.enum_value().ok()? != Suffix::Term
        || parent.suffix.enum_value().ok()? != Suffix::Type
    {
        return None;
    }
    let owner = symbol
        .strip_suffix('.')?
        .rfind('#')
        .map(|i| &symbol[..=i])?;
    Some((owner.to_string(), last.name.clone()))
}

/// Roadmap issue #32 v5 (v0.4.0): one Function → Type "uses" edge
/// derived from a SCIP reference occurrence whose descriptor is a
/// Type (suffix `#`) and whose source line falls inside a function
/// body. Deduped per (function, type) pair at parse time so the
/// resulting edge table has at most one edge per pair regardless
/// of how many times the type is referenced in the function body —
/// matches `METHOD_OF`'s single-edge-per-pair shape and keeps query
/// joins cheap.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UsesTypeFact {
    /// SCIP symbol of the function whose body contains the type
    /// reference.
    pub function_symbol: String,
    /// SCIP symbol of the referenced type. Cross-file types just
    /// won't get a the store edge (the MATCH on the Type table fails
    /// silently) — same convention as `CALLS`.
    pub type_symbol: String,
}

/// Aggregate produced by `parse_scip`.
#[derive(Debug, Default)]
pub struct ScipFacts {
    pub functions: Vec<FunctionFact>,
    /// Function → Function call edges derived from SCIP reference
    /// occurrences. Each emitted edge has its caller resolved via
    /// `enclosing_range` (or, when the indexer omits it, by sorting
    /// definitions in the same file and treating each function's body
    /// as the span up to the next definition). See roadmap issue #9.
    pub calls: Vec<CallFact>,
    /// Roadmap issue #32 v5 (v0.4.0): Function → Type "uses" edges
    /// derived from per-document occurrence walks. Deduped per
    /// (function, type) pair at parse time. See `UsesTypeFact`.
    pub uses_types: Vec<UsesTypeFact>,
    /// Field / property definitions (`Type#name.`). See [`FieldFact`].
    pub fields: Vec<FieldFact>,
    /// Function → Field references, deduped per pair. See
    /// [`ReferenceFact`].
    pub references: Vec<ReferenceFact>,
    /// Roadmap issue #32 v6 (v0.4.0): Type → Type "implements" edges
    /// derived from `SymbolInformation.relationships` entries with
    /// `is_implementation: true` whose FROM/TO kind shape doesn't
    /// match the EXTENDS rule. Deduped per (from, to) pair. See
    /// `ImplementsFact`.
    pub implements: Vec<ImplementsFact>,
    /// Roadmap issue #32 v8 (v0.4.0): Type → Type "extends" edges —
    /// the inheritance subset of the same `is_implementation`
    /// relationships, classified via the global symbol → kind map
    /// built after every Document is walked. See `ExtendsFact`.
    pub extends: Vec<ExtendsFact>,
    /// File → File import edges derived from SCIP occurrences
    /// with the Import role bit (= 2) set. Pre-aggregated per-
    /// (importer, imported) pair downstream in `imports_ingest`;
    /// duplicate ImportFacts here (same pair from multiple
    /// occurrences) are intentional — the aggregation counts
    /// them. See roadmap issue #16.
    pub imports: Vec<ImportFact>,
    /// Count of symbols whose kind was filled in by descriptor-suffix
    /// fallback because the indexer emitted `UnspecifiedKind`. Non-zero
    /// implies the SCIP file was produced by a kind-omitting indexer
    /// (e.g. scip-python 0.6.x); surfacing the number in CLI output makes
    /// the fallback visible rather than silent.
    pub inferred_count: usize,
    /// Java methods / constructors the compiler generated (Lombok,
    /// records, enums) — present in SCIP, absent from the source. They
    /// become `generated` Function rows: call targets, never callers, and
    /// outside every coverage measure.
    pub generated_count: usize,
}

/// Parse a SCIP index file at `scip_path` into a `ScipFacts` aggregate.
///
/// `root` is the repo root used to anchor relative file paths. The SCIP
/// format already emits paths relative to the project root — we accept
/// the parameter for forward-compat (e.g. if a future indexer emits
/// absolute paths or paths anchored to a different root).
///
/// Errors:
///   - I/O reading the file
///   - protobuf decoding (corrupt or non-SCIP bytes)
pub fn parse_scip(scip_path: &Path, root: &Path) -> Result<ScipFacts> {
    let bytes = std::fs::read(scip_path)
        .with_context(|| format!("read scip file {}", scip_path.display()))?;
    if bytes.is_empty() {
        bail!("scip file {} is empty", scip_path.display());
    }
    let mut index = Index::parse_from_bytes(&bytes)
        .with_context(|| format!("decode scip protobuf at {}", scip_path.display()))?;

    let mut facts = ScipFacts::default();

    // scip-java gives compiler-generated members (Lombok accessors,
    // record accessors, enum `values()`) a Definition occurrence at a call
    // site, the class name or the annotation. Demote those so they are not
    // caller intervals; they still become (`generated`) Function rows.
    for doc in &mut index.documents {
        if !doc.relative_path.ends_with(".java") {
            continue;
        }
        let path = root.join(normalise_relative_path(&doc.relative_path));
        if let Ok(content) = std::fs::read_to_string(&path) {
            facts.generated_count += demote_generated_java_definitions(doc, &content);
        }
    }

    // Roadmap issue #32 v8 (v0.4.0): the EXTENDS / IMPLEMENTS split
    // needs to know the TO-side kind to classify each
    // `is_implementation` relationship, but the TO may live in a
    // different Document than the FROM. Collect raw triples per
    // document during the main walk, classify after the global
    // symbol → kind map is complete. Sized for the common case
    // (most repos have <500 relationships); reallocations are
    // cheap compared to the SCIP parse itself.
    let mut raw_relationships: Vec<RawRelationshipFact> = Vec::new();

    // Roadmap issue #26: build the file → last-touched-timestamp
    // map once. Single `git log` walk over the whole repo —
    // O(commits + files) total, beats per-file `git log -1` by
    // ~50x on mid-sized Python repos. Empty map when not in a git
    // repo; downstream `last_touched` falls through with empty
    // string, non-fatal.
    let mtime_map = git_file_mtimes(root);

    // Function-valued variables (`export const f = () => …`) don't end in
    // `().`, so a call to one is only recognisable by symbol; collect them
    // across documents first, since the callee may be defined elsewhere.
    let function_variables: std::collections::HashSet<&str> = index
        .documents
        .iter()
        .flat_map(|d| &d.symbols)
        .filter(|s| is_function_valued_variable(s))
        .map(|s| s.symbol.as_str())
        .collect();

    let module_files = module_files(&index);
    for doc in &index.documents {
        let file = normalise_relative_path(&doc.relative_path);

        // Build a map: symbol -> (start_line, end_line) from definition
        // occurrences. SCIP attaches doc-comments to SymbolInformation
        // (in `documentation`) but the *line* of the definition lives on
        // the Occurrence with role bit 1 (Definition) set.
        let definition_lines: std::collections::HashMap<&str, u32> = doc
            .occurrences
            .iter()
            .filter(|occ| occ.symbol_roles & 1 == 1)
            .map(|occ| {
                let line = occ.range.first().copied().unwrap_or(0);
                let line_1based = if line < 0 { 0 } else { line as u32 + 1 };
                (occ.symbol.as_str(), line_1based)
            })
            .collect();

        // Roadmap issue #9: derive CALLS edges from this document's
        // reference occurrences. Caller resolution uses each function-
        // kind definition's `enclosing_range` (the SCIP body span);
        // when the indexer omits that field, fall back to sorting
        // definitions by start line and using each one's span up to
        // the next definition's start. See `derive_calls_for_document`.
        derive_calls_for_document(doc, &file, &function_variables, &mut facts.calls);

        // Roadmap issue #32 v5 (v0.4.0): derive USES_TYPE edges from
        // this document's type-suffix reference occurrences. Same
        // caller-interval shape as CALLS, but filtered for SCIP
        // descriptors whose last suffix is `Type` (i.e. ends with
        // `#`).
        derive_uses_type_for_document(doc, &mut facts.uses_types);
        derive_references_for_document(doc, &file, &mut facts.references);

        // Roadmap issue #32 v6 / v8 (v0.4.0): collect raw
        // is_implementation relationships from every type-kind
        // SymbolInformation. The split into IMPLEMENTS vs EXTENDS
        // runs post-loop once every Document's SymbolInformation
        // (and thus every TO-side kind) has been seen.
        collect_raw_relationships_for_document(doc, &mut raw_relationships);

        // Roadmap issue #16: derive IMPORTS edges from this
        // document's Import-role occurrences. SCIP marks the
        // import-statement reference sites with the Import
        // bit (= 2); pulling the importee's file out of the
        // symbol descriptor gives File→File granularity.
        derive_imports_for_document(doc, &file, root, &module_files, &mut facts.imports);

        // Roadmap issue #22: read the file once per Document so
        // `body_excerpt` extraction is O(files) not O(symbols). A
        // missing / unreadable file leaves `None`, which causes every
        // symbol in this Document to fall through with an empty
        // body_excerpt — non-fatal, agents fall back to a Read call.
        let file_content: Option<String> = std::fs::read_to_string(root.join(&file)).ok();

        for sym in &doc.symbols {
            // Fields go to their own table, ahead of the kind filter: their
            // kind is `Field`/`Property` or unset, both of which it drops.
            if let Some((owner, name)) = field_parts(&sym.symbol) {
                facts.fields.push(FieldFact {
                    line: definition_lines
                        .get(sym.symbol.as_str())
                        .copied()
                        .unwrap_or(0),
                    symbol: sym.symbol.clone(),
                    name,
                    owner,
                    file: file.clone(),
                    doc_comment: join_doc(&sym.documentation),
                    language: language_from_file(&file).to_string(),
                });
                continue;
            }
            // Filter to kinds we care about. `kind` is `EnumOrUnknown`;
            // unknown variants get `Other`.
            // scip-python 0.6.x (and scip-typescript / scip-dotnet) leave
            // kind unset. `resolve_symbol_kind` recovers it from the
            // descriptor suffix, or from a TS hover type, before the
            // "Other" drop swallows the entire file.
            let kind = resolve_symbol_kind(sym);
            let scip_kind = sym.kind.enum_value().unwrap_or(ScipKind::UnspecifiedKind);
            if scip_kind == ScipKind::UnspecifiedKind && !matches!(kind, FunctionKind::Other) {
                facts.inferred_count += 1;
            }
            // `Other` covers locals, parameters, fields, etc. — too noisy
            // for the graph. Skip them. A `local N` closure is skipped even
            // when typed as a function: local symbols are only unique within
            // one document, so as Function rows they collide across files
            // (and its calls are attributed to the enclosing definition).
            if matches!(kind, FunctionKind::Other) || sym.symbol.starts_with("local ") {
                continue;
            }
            // A Java method with no Definition left was generated by the
            // compiler; see `demote_generated_java_definitions`.
            let generated = file.ends_with(".java")
                && matches!(kind, FunctionKind::Function | FunctionKind::Method)
                && !definition_lines.contains_key(sym.symbol.as_str());

            let symbol = sym.symbol.clone();
            let crate_name = extract_crate_name(&symbol)
                .unwrap_or_else(|| crate_name_from_file(&file).unwrap_or_default());
            let line = definition_lines.get(symbol.as_str()).copied().unwrap_or(0);
            let doc_comment = join_doc(&sym.documentation);
            let language = language_from_file(&file).to_string();
            // Roadmap issue #22: signature from SCIP's
            // `signature_documentation` Document.text when the
            // indexer provided it. scip-python and older
            // rust-analyzer builds omit this — empty signature is
            // not an error.
            //
            // Per interrogation-037/038 + iter-191 closure:
            // scip-python leaves signature_documentation empty
            // 100% of the time but body_excerpt's first line is
            // the `def function_name(args):` declaration. Fall
            // back to that when signature is empty so Function-
            // signature dedup, the iter-189 diagnostic, and any
            // signature-projecting query (functions-in-file etc.)
            // surface meaningful Python signatures. NOOP for
            // languages where SCIP did provide a real signature.
            let signature = sym
                .signature_documentation
                .as_ref()
                .map(|d| d.text.clone())
                .unwrap_or_default();
            // Roadmap issue #22: first ~15 lines of the body
            // starting at `line`, capped at 500 bytes. Empty when
            // the file couldn't be read or line is 0 (unknown).
            let body_excerpt = if generated {
                String::new() // no body of its own; line 0 would read the file head
            } else {
                extract_body_excerpt(file_content.as_deref(), line)
            };
            // Per iter-191 closure: derive signature from
            // body_excerpt's first non-blank line when SCIP left
            // it empty. Catches the scip-python upstream gap
            // documented in feedback_function_signature_column_
            // empty_py without changing existing-signature
            // behavior for Rust / langs with populated SCIP
            // signature data.
            let signature = if signature.is_empty() {
                derive_signature_from_body(&body_excerpt)
            } else {
                signature
            };
            // Roadmap issue #26: git author-timestamp of the most
            // recent commit touching this file. Empty when the
            // repo isn't a git worktree.
            let last_touched = mtime_map.get(&file).cloned().unwrap_or_default();

            facts.functions.push(FunctionFact {
                symbol,
                crate_name,
                file: file.clone(),
                line,
                doc_comment,
                kind,
                language,
                signature,
                body_excerpt,
                last_touched,
                generated,
            });
        }
    }

    // Roadmap issue #32 v8 (v0.4.0): classify the raw
    // is_implementation relationships collected per-doc above.
    // Now that every Document has been walked, build the global
    // symbol → kind map and split each raw triple into either
    // ExtendsFact (trait inheriting trait) or ImplementsFact
    // (everything else).
    classify_raw_relationships(&index, raw_relationships, &mut facts);
    add_dispatch_calls(&index, &mut facts.calls);

    Ok(facts)
}

/// Walk one SCIP `Document` and emit a `CallFact` for every reference
/// occurrence whose containing function we can resolve. Pushed onto
/// the shared `calls` buffer rather than returned so the caller's
/// per-document loop stays flat.
///
/// Resolution algorithm:
///   1. Walk `doc.symbols` to find function-kind symbols (Function /
///      Method, including the `UnspecifiedKind` → descriptor-suffix
///      fallback for scip-python and friends). Build a set.
///   2. For each Definition occurrence of a function-kind symbol,
///      derive a `[start_line, end_line]` interval. Prefer the
///      occurrence's `enclosing_range` (SCIP's body span); when empty,
///      use the next definition's start line - 1, falling back to
///      `i32::MAX` for the trailing definition.
///   3. For each Reference occurrence (no Definition role bit) whose
///      symbol ends with `().` (the SCIP shape for a function /
///      method reference), find the innermost interval containing
///      its line. Emit one `CallFact` with caller = that interval's
///      symbol, callee = the reference's symbol. Self-edges (caller
///      == callee) are skipped — recursion shows up at the function
///      level as a loop-back edge later if we ever want it.
/// Roadmap issue #16: walk one Document's occurrences for Import-
/// role hits and emit one `ImportFact` per (importer, imported)
/// site. Aggregation into a single per-pair `import_count`
/// happens at edge-emit time in `imports_ingest`; this pass
/// emits raw facts so the counter is accurate.
fn derive_imports_for_document(
    doc: &scip::types::Document,
    importer_file: &str,
    root: &Path,
    module_files: &HashMap<String, String>,
    imports: &mut Vec<ImportFact>,
) {
    // #227: scip-python never sets the Import role (bit 2). For a
    // document with no Import-role occurrence, the references on its
    // `import` / `from … import` lines are its imports.
    let has_import_role = doc.occurrences.iter().any(|o| o.symbol_roles & 2 != 0);
    let import_lines: HashSet<usize> = if has_import_role {
        HashSet::new()
    } else {
        std::fs::read_to_string(root.join(importer_file))
            .unwrap_or_default()
            .lines()
            .enumerate()
            .filter(|(_, l)| {
                let l = l.trim_start();
                l.starts_with("import ") || l.starts_with("from ")
            })
            .map(|(i, _)| i)
            .collect()
    };
    for occ in &doc.occurrences {
        let is_import = if has_import_role {
            // bit 1 (Import = 2). A single occurrence can be both
            // Import and Definition (rare); it still counts.
            occ.symbol_roles & 2 != 0
        } else {
            occ.symbol_roles & 1 == 0
                && occ
                    .range
                    .first()
                    .and_then(|l| usize::try_from(*l).ok())
                    .is_some_and(|l| import_lines.contains(&l))
        };
        if !is_import {
            continue;
        }
        let imported_file = module_of(&occ.symbol)
            .and_then(|m| module_files.get(&m).cloned())
            .or_else(|| file_from_scip_symbol(&occ.symbol))
            .unwrap_or_default();
        if imported_file.is_empty() || imported_file == importer_file {
            // Skip self-imports (`from . import foo` resolving
            // back to the same file) — those are noise.
            continue;
        }
        imports.push(ImportFact {
            importer_file: importer_file.to_string(),
            imported_file,
        });
    }
}

/// #227: the module a symbol lives in — its first Namespace / Package
/// descriptor (scip-python: `` `pkg.hparams`/hparams. `` → `pkg.hparams`).
fn module_of(symbol: &str) -> Option<String> {
    use scip::types::descriptor::Suffix;
    scip::symbol::parse_symbol(symbol)
        .ok()?
        .descriptors
        .into_iter()
        .find(|d| {
            matches!(
                d.suffix.enum_value(),
                Ok(Suffix::Namespace | Suffix::Package)
            )
        })
        .map(|d| d.name)
}

/// #227: module name → the document that defines it, from each
/// document's definitions. scip-python module descriptors carry no file
/// path, so this is how an import of `pkg.hparams` finds
/// `pkg/hparams.py`. The first document defining a module wins.
fn module_files(index: &Index) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for doc in &index.documents {
        for occ in doc.occurrences.iter().filter(|o| o.symbol_roles & 1 == 1) {
            if let Some(module) = module_of(&occ.symbol) {
                out.entry(module)
                    .or_insert_with(|| normalise_relative_path(&doc.relative_path));
            }
        }
    }
    out
}

/// Extract the file path from a SCIP symbol string. SCIP symbols
/// take the shape:
///
/// ```text
/// scheme manager package version <descriptor>...
/// ```
///
/// where the descriptor sequence often begins with the file
/// path (e.g. `src/foo.py/Bar#method().`). We parse via the
/// official `scip::symbol::parse_symbol` and inspect the
/// descriptors for one with a `Namespace` suffix that looks
/// like a relative file path (`.py`, `.ts`, `.rs`, etc.). The
/// first such descriptor's name is the importer's file.
///
/// Returns `None` when no file-like descriptor is present —
/// common for stdlib references (`scheme stdlib`) and local
/// symbols (`local 0`), both of which we want to skip at the
/// IMPORTS-edge level anyway.
fn file_from_scip_symbol(symbol: &str) -> Option<String> {
    use scip::types::descriptor::Suffix;
    let parsed = scip::symbol::parse_symbol(symbol).ok()?;
    // Walk descriptors looking for a Namespace-suffix whose name
    // contains a `/` (a file path) or ends with a recognised
    // source extension. SCIP's scip-python encodes file paths
    // as Namespace descriptors; rust-analyzer uses them too.
    for desc in &parsed.descriptors {
        let suffix = desc.suffix.enum_value().ok();
        let is_namespace = matches!(suffix, Some(Suffix::Namespace | Suffix::Package));
        if !is_namespace {
            continue;
        }
        if looks_like_source_path(&desc.name) {
            return Some(desc.name.clone());
        }
    }
    None
}

/// Heuristic: does this descriptor name look like a repo-
/// relative source file path? Either it contains a `/` (so it's
/// a multi-component path), or it ends with a recognised
/// source-file extension. Stdlib-style descriptors (`builtins`,
/// `typing`) fail both predicates and get rejected.
fn looks_like_source_path(name: &str) -> bool {
    if name.contains('/') {
        return true;
    }
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext,
        "rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs"
    )
}

fn derive_calls_for_document(
    doc: &scip::types::Document,
    file: &str,
    function_variables: &std::collections::HashSet<&str>,
    calls: &mut Vec<CallFact>,
) {
    let intervals = caller_intervals_for_document(doc);
    if intervals.is_empty() {
        return;
    }

    for occ in &doc.occurrences {
        if occ.symbol_roles & 1 == 1 {
            continue;
        }
        // A method descriptor is `name(<disambiguator>).`: `foo().` for the
        // first overload, `foo(+1).` for the next. Matching only `().`
        // dropped every call to a second or later overload.
        if !occ.symbol.ends_with(").") && !function_variables.contains(occ.symbol.as_str()) {
            continue;
        }
        let ref_line = occ.range.first().copied().unwrap_or(0);
        let Some(caller) = innermost_caller_for_line(ref_line, &intervals) else {
            continue;
        };
        if caller == occ.symbol {
            continue;
        }
        let line_1based = if ref_line < 0 { 0 } else { ref_line as u32 + 1 };
        calls.push(CallFact {
            caller_symbol: caller.to_string(),
            callee_symbol: occ.symbol.clone(),
            source_file: file.to_string(),
            source_line: line_1based,
            dispatch: false,
        });
    }
}

/// #268: a call through an interface or abstract method lands on that
/// method, never on its implementations. For every method that
/// declares itself an implementation of a method defined in this index
/// (SCIP `is_implementation` on a method symbol), add a `dispatch` call
/// from each caller of the overridden method to the implementation.
/// Overrides of methods outside the index (`Object#toString`) are left
/// alone: every `toString()` call would otherwise reach every override.
fn add_dispatch_calls(index: &Index, calls: &mut Vec<CallFact>) {
    let defined: HashSet<&str> = index
        .documents
        .iter()
        .flat_map(|d| &d.symbols)
        .map(|s| s.symbol.as_str())
        .collect();
    let mut implementations: HashMap<&str, Vec<&str>> = HashMap::new();
    for sym in index.documents.iter().flat_map(|d| &d.symbols) {
        if !sym.symbol.ends_with(").") {
            continue;
        }
        for rel in &sym.relationships {
            if rel.is_implementation
                && rel.symbol != sym.symbol
                && rel.symbol.ends_with(").")
                && defined.contains(rel.symbol.as_str())
            {
                implementations
                    .entry(rel.symbol.as_str())
                    .or_default()
                    .push(sym.symbol.as_str());
            }
        }
    }
    let dispatched: Vec<CallFact> = calls
        .iter()
        .flat_map(|call| {
            implementations
                .get(call.callee_symbol.as_str())
                .into_iter()
                .flatten()
                .filter(|imp| **imp != call.caller_symbol)
                .map(|imp| CallFact {
                    callee_symbol: (*imp).to_string(),
                    dispatch: true,
                    ..call.clone()
                })
        })
        .collect();
    calls.extend(dispatched);
}

/// Legacy gap 7: one `ReferenceFact` per (enclosing function, field)
/// for every non-definition occurrence of a field symbol, resolved
/// through the same intervals as CALLS (so closures count towards their
/// enclosing function). Keeps the first reference's line.
fn derive_references_for_document(
    doc: &scip::types::Document,
    file: &str,
    refs: &mut Vec<ReferenceFact>,
) {
    use std::collections::HashSet;
    let intervals = caller_intervals_for_document(doc);
    if intervals.is_empty() {
        return;
    }
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    for occ in &doc.occurrences {
        if occ.symbol_roles & 1 == 1 || field_parts(&occ.symbol).is_none() {
            continue;
        }
        let ref_line = occ.range.first().copied().unwrap_or(0);
        let Some(caller) = innermost_caller_for_line(ref_line, &intervals) else {
            continue;
        };
        if !seen.insert((caller, occ.symbol.as_str())) {
            continue;
        }
        refs.push(ReferenceFact {
            function_symbol: caller.to_string(),
            field_symbol: occ.symbol.clone(),
            source_file: file.to_string(),
            source_line: u32::try_from(ref_line).map_or(0, |l| l + 1),
        });
    }
}

/// Roadmap issue #32 v5 (v0.4.0): walk one Document's occurrences,
/// resolve each type-suffix reference to the enclosing function via
/// the shared `caller_intervals_for_document` machinery, and emit a
/// deduped `UsesTypeFact` per (function, type) pair.
///
/// The TO-side filter is purely descriptor-shape (`Suffix::Type` →
/// trailing `#`); we don't check that the type is defined in this
/// SCIP index — the edge insert silently drops edges whose
/// target isn't in the `Type` table, same as `CALLS`. Cross-crate /
/// stdlib type references therefore land as no-op SCIP facts but
/// don't pollute the rel table.
fn derive_uses_type_for_document(doc: &scip::types::Document, uses: &mut Vec<UsesTypeFact>) {
    use std::collections::HashSet;

    let intervals = caller_intervals_for_document(doc);
    if intervals.is_empty() {
        return;
    }

    // Per-document dedupe: a function can reference the same type
    // many times in its body (parameter type, local binding, cast,
    // turbofish, ...). The edge table is a single edge per pair,
    // so collapse here rather than letting `insert_uses_type` re-do
    // the work N times via `MERGE` (which the store doesn't have anyway).
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    for occ in &doc.occurrences {
        if occ.symbol_roles & 1 == 1 {
            continue;
        }
        if !is_type_descriptor(&occ.symbol) {
            continue;
        }
        let ref_line = occ.range.first().copied().unwrap_or(0);
        let Some(caller) = innermost_caller_for_line(ref_line, &intervals) else {
            continue;
        };
        if caller == occ.symbol {
            continue;
        }
        if !seen.insert((caller, occ.symbol.as_str())) {
            continue;
        }
        uses.push(UsesTypeFact {
            function_symbol: caller.to_string(),
            type_symbol: occ.symbol.clone(),
        });
    }
}

/// Roadmap issue #32 v8 (v0.4.0): one collected
/// `is_implementation` relationship triple waiting on the global
/// kind map to classify it. The FROM kind is already known (read
/// off the subject's `SymbolInformation`); the TO kind is filled in
/// later in `classify_raw_relationships`.
#[derive(Debug, Clone)]
struct RawRelationshipFact {
    from_symbol: String,
    from_kind: FunctionKind,
    to_symbol: String,
}

/// Roadmap issue #32 v6 / v8 (v0.4.0): walk every
/// `SymbolInformation` in a Document and append one raw triple per
/// `is_implementation: true` relationship whose FROM subject is a
/// type-kind symbol. FROM-side kind filter keeps non-type subjects
/// (modules, free functions) out — only `Struct` / `Enum` /
/// `Trait` / `TypeAlias` types participate. TO-side existence in
/// the graph is checked later by the edge insert.
///
/// Classification (EXTENDS vs IMPLEMENTS) is deferred to the
/// post-walk `classify_raw_relationships` pass because the TO may
/// live in a different Document.
fn collect_raw_relationships_for_document(
    doc: &scip::types::Document,
    raw: &mut Vec<RawRelationshipFact>,
) {
    for sym in &doc.symbols {
        let kind = resolve_symbol_kind(sym);
        let is_type = matches!(
            kind,
            FunctionKind::Struct
                | FunctionKind::Enum
                | FunctionKind::Trait
                | FunctionKind::TypeAlias
        );
        if !is_type {
            continue;
        }
        for rel in &sym.relationships {
            if !rel.is_implementation {
                continue;
            }
            if rel.symbol.is_empty() || rel.symbol == sym.symbol {
                continue;
            }
            raw.push(RawRelationshipFact {
                from_symbol: sym.symbol.clone(),
                from_kind: kind,
                to_symbol: rel.symbol.clone(),
            });
        }
    }
}

/// Roadmap issue #32 v8 (v0.4.0): post-walk classifier. Builds a
/// global `symbol → FunctionKind` map across every Document in the
/// SCIP index, then drains the per-doc raw relationship buffer into
/// either `facts.extends` or `facts.implements` based on the
/// FROM/TO kind pair.
///
/// Rule (intentionally narrow — see `ExtendsFact` for rationale):
/// - FROM kind is `Trait` AND TO kind is `Trait` → EXTENDS
/// - otherwise → IMPLEMENTS
///
/// Global dedupe on `(from, to)` keeps the per-doc dedupe honest
/// across multi-document indexes (e.g. workspace re-exports).
fn classify_raw_relationships(index: &Index, raw: Vec<RawRelationshipFact>, facts: &mut ScipFacts) {
    use std::collections::{HashMap, HashSet};

    // Build the global kind map. Same scip → FunctionKind mapping
    // the per-doc walk uses, so a symbol declared in two docs gets
    // a stable kind (first wins; in practice the SCIP indexer emits
    // identical kind for re-exported symbols).
    let mut kind_by_symbol: HashMap<&str, FunctionKind> = HashMap::new();
    for doc in &index.documents {
        for sym in &doc.symbols {
            let kind = resolve_symbol_kind(sym);
            if matches!(kind, FunctionKind::Other) {
                continue;
            }
            kind_by_symbol.entry(sym.symbol.as_str()).or_insert(kind);
        }
    }

    let mut implements_seen: HashSet<(String, String)> = HashSet::new();
    let mut extends_seen: HashSet<(String, String)> = HashSet::new();
    for fact in raw {
        // Cross-crate / stdlib refs leave to_kind = None; default
        // to IMPLEMENTS, the more common case for "unknown trait".
        let to_kind = kind_by_symbol.get(fact.to_symbol.as_str()).copied();
        let is_extends = matches!(
            (fact.from_kind, to_kind),
            (FunctionKind::Trait, Some(FunctionKind::Trait))
        );
        if is_extends {
            let key = (fact.from_symbol.clone(), fact.to_symbol.clone());
            if extends_seen.insert(key) {
                facts.extends.push(ExtendsFact {
                    from_type: fact.from_symbol,
                    to_type: fact.to_symbol,
                });
            }
        } else {
            let key = (fact.from_symbol.clone(), fact.to_symbol.clone());
            if implements_seen.insert(key) {
                facts.implements.push(ImplementsFact {
                    from_type: fact.from_symbol,
                    to_type: fact.to_symbol,
                });
            }
        }
    }
}

/// Shared kind resolution: prefer the SCIP-reported kind, fall
/// back to descriptor-suffix inference when the indexer left it
/// unset (scip-python 0.6.x). Pulled out so
/// `collect_raw_relationships_for_document` and
/// `classify_raw_relationships` share one source of truth.
fn resolve_symbol_kind(sym: &scip::types::SymbolInformation) -> FunctionKind {
    let scip_kind = sym.kind.enum_value().unwrap_or(ScipKind::UnspecifiedKind);
    let mut kind = FunctionKind::from_scip(scip_kind);
    if matches!(kind, FunctionKind::Other) && scip_kind == ScipKind::UnspecifiedKind {
        if let Some(inferred) = infer_kind_from_descriptor(&sym.symbol) {
            kind = inferred;
        } else if is_function_valued_variable(sym) {
            kind = FunctionKind::Function;
        }
    }
    kind
}

/// scip-typescript records `export const f = (x) => …` and
/// `const f = function (…) {…}` as plain terms (`api.ts/f.`) with no kind;
/// only the hover text (`var f: (x: number) => number`) says the value is
/// a function. Such a variable is a function for the graph, so calls and
/// field reads inside it get a caller. Fields (`Type#name.`) are excluded.
fn is_function_valued_variable(sym: &scip::types::SymbolInformation) -> bool {
    use scip::types::descriptor::Suffix;
    static HOVER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    if sym.symbol.starts_with("local ") || field_parts(&sym.symbol).is_some() {
        return false;
    }
    let is_term = scip::symbol::parse_symbol(&sym.symbol)
        .ok()
        .and_then(|p| p.descriptors.last().map(|d| d.suffix.enum_value()))
        .is_some_and(|s| s == Ok(Suffix::Term));
    if !is_term {
        return false;
    }
    #[allow(
        clippy::unwrap_used,
        reason = "literal regex; compile is infallible at runtime"
    )]
    let hover = HOVER.get_or_init(|| {
        regex::Regex::new(r"(?m)^(?:var|let|const) [\w$]+\s*:\s*(?:<[^>]*>\s*)?\(.*\)\s*=>")
            .unwrap()
    });
    sym.documentation.first().is_some_and(|d| hover.is_match(d))
}

/// Clears the Definition role on every Java method / constructor
/// definition that is not a declaration in `content`, returning how many
/// symbols were demoted. A declared method's definition range covers its
/// own name, followed by `(` and not preceded by `.`, `new` or `return`;
/// scip-java puts a generated member's definition on a call site
/// (`.setJobId(`), the class name, or the annotation that generated it.
fn demote_generated_java_definitions(doc: &mut scip::types::Document, content: &str) -> usize {
    let lines: Vec<&str> = content.lines().collect();
    // Lombok marks every member it writes `@Generated` (lombok.Generated),
    // and SemanticDB keeps it in the signature. That is decisive, and it is
    // the only way to tell a generated getter that shares its name with a
    // hand-written overload: SemanticDB puts both definitions on the
    // hand-written method's name.
    let mut annotated: std::collections::HashSet<String> = doc
        .symbols
        .iter()
        .filter(|s| {
            s.signature_documentation
                .as_ref()
                .is_some_and(|d| has_generated_annotation(&d.text))
        })
        .map(|s| s.symbol.clone())
        .collect();
    let mut generated: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut declared: std::collections::HashSet<String> = std::collections::HashSet::new();
    for occ in &doc.occurrences {
        if occ.symbol_roles & 1 != 1 || !occ.symbol.ends_with(").") {
            continue;
        }
        if annotated.contains(&occ.symbol) {
            continue;
        }
        // Records' accessors and enums' values() carry no annotation; their
        // definition never sits on a declaration of their own.
        if is_java_declaration(&lines, &occ.range, &occ.symbol) {
            declared.insert(occ.symbol.clone());
        } else {
            generated.insert(occ.symbol.clone());
        }
    }
    generated.retain(|s| !declared.contains(s));
    annotated.retain(|s| s.ends_with(")."));
    generated.extend(annotated);
    for occ in &mut doc.occurrences {
        if occ.symbol_roles & 1 == 1 && generated.contains(&occ.symbol) {
            occ.symbol_roles &= !1;
        }
    }
    generated.len()
}

/// Whether a SemanticDB signature carries a `@Generated` annotation,
/// bare or qualified (`@lombok.Generated`, `@javax.annotation.Generated`),
/// as a whole name: JPA's `@GeneratedValue` is a hand-written member.
/// Checked-in codegen output marked `@Generated` is demoted as well.
fn has_generated_annotation(signature: &str) -> bool {
    signature.match_indices("Generated").any(|(i, m)| {
        let before = signature[..i].chars().next_back();
        let after = signature[i + m.len()..].chars().next();
        matches!(before, Some('@' | '.')) && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

/// Whether a Definition `range` (`[line, start, end]`, single-line) sits
/// on the declared name of the method `symbol` in `lines`. SemanticDB
/// places the definition on the first occurrence of the name in the
/// declaration's span, which can be a string in one of its annotations
/// (`@Operation(operationId = "retrieveLoan")`, `@Path("/template")`) or a
/// comment between the annotations; that counts when the declaration
/// itself follows. A generated member's definition is never inside a
/// string or a comment.
fn is_java_declaration(lines: &[&str], range: &[i32], symbol: &str) -> bool {
    let [line, start, end] = match *range {
        [l, s, e] => [l, s, e],
        _ => return false, // a multi-line range spans an annotation, not a name
    };
    let (Ok(line), Ok(start), Ok(end)) = (
        usize::try_from(line),
        usize::try_from(start),
        usize::try_from(end),
    ) else {
        return false;
    };
    let Some(text) = lines.get(line) else {
        return false;
    };
    // `pkg/Outer#Inner#method(+1).` → `method`; `` Type#`<init>`(). `` → `Type`.
    let descriptor = symbol.trim_end_matches('.');
    let descriptor = &descriptor[..descriptor.rfind('(').unwrap_or(descriptor.len())];
    let mut parts = descriptor.rsplit(['#', '/']);
    let mut expected = parts.next().unwrap_or_default();
    if expected == "`<init>`" {
        expected = parts.next().unwrap_or_default();
    }
    let name: Vec<char> = expected.chars().collect();
    let chars: Vec<char> = text.chars().collect();
    if name.is_empty() || chars.get(start..end) != Some(&name[..]) {
        return false;
    }
    if declares_at(&chars, start, end) {
        return true;
    }
    // Inside a string literal (an odd number of unescaped quotes before
    // it) or a comment (`//`, or a `/*` / `*` comment line).
    let mut in_string = false;
    let mut in_comment = {
        let t = text.trim_start();
        t.starts_with("/*") || t.starts_with('*')
    };
    let mut escaped = false;
    for (i, c) in chars[..start].iter().enumerate() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => in_string = !in_string,
            '/' if !in_string && chars.get(i + 1) == Some(&'/') => in_comment = true,
            _ => {}
        }
    }
    // The annotations before a declaration can carry a long `"""` text
    // block, so look well ahead.
    (in_string || in_comment)
        && lines.iter().skip(line + 1).take(80).any(|l| {
            let chars: Vec<char> = l.chars().collect();
            (0..chars.len()).any(|i| {
                chars[i..].starts_with(&name)
                    && !(i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_'))
                    && declares_at(&chars, i, i + name.len())
            })
        })
}

/// Whether the name at `chars[start..end]` is declared there: followed by
/// `(`, and preceded by a type or modifier, not by `.`, `"`, `new` or
/// `return`.
fn declares_at(chars: &[char], start: usize, end: usize) -> bool {
    let after: String = chars[end..].iter().collect();
    if !after.trim_start().starts_with('(') {
        return false;
    }
    let before: String = chars[..start].iter().collect();
    let before = before.trim_end();
    !(before.ends_with('.')
        || before.ends_with('"')
        || ["new", "return", "throw", "else", "case", "yield"]
            .iter()
            .any(|kw| {
                before.ends_with(kw)
                    && !before[..before.len() - kw.len()]
                        .ends_with(|c: char| c.is_alphanumeric() || c == '_')
            }))
}

/// Build the `[start_line, end_line, caller_symbol]` interval list
/// over a Document's function-kind definition occurrences. Shared by
/// `derive_calls_for_document` (issue #9) and
/// `derive_uses_type_for_document` (issue #32 v5).
///
/// Definition lines come from the occurrence's `range[0]`; body-end
/// lines prefer the SCIP `enclosing_range` (whose shape is
/// `[startLine, startChar, endLine, endChar]` or
/// `[startLine, startChar, endChar]` for single-line bodies). When
/// the indexer omits `enclosing_range` (older scip-python, some
/// rust-analyzer builds), we fall back to "next definition's start
/// line - 1" via a sorted-by-start linear scan.
fn caller_intervals_for_document(doc: &scip::types::Document) -> Vec<(i32, i32, &str)> {
    use std::collections::HashSet;

    let mut function_kind_symbols: HashSet<&str> = HashSet::new();
    for sym in &doc.symbols {
        let kind = resolve_symbol_kind(sym);
        if matches!(kind, FunctionKind::Function | FunctionKind::Method) {
            function_kind_symbols.insert(sym.symbol.as_str());
        }
    }
    if function_kind_symbols.is_empty() {
        return Vec::new();
    }

    let mut intervals: Vec<(i32, i32, &str)> = Vec::new();
    for occ in &doc.occurrences {
        if occ.symbol_roles & 1 != 1 {
            continue;
        }
        if !function_kind_symbols.contains(occ.symbol.as_str()) {
            continue;
        }
        // A `local N` symbol is a closure / lambda (scip_dart, scip-typescript).
        // It has no file or name to report, so its calls belong to the
        // enclosing named definition: leaving it out of the intervals makes
        // that definition the innermost match.
        if occ.symbol.starts_with("local ") {
            continue;
        }
        let start = occ.range.first().copied().unwrap_or(0);
        let end = match occ.enclosing_range.len() {
            4 => occ.enclosing_range[2],
            3 => occ.enclosing_range[0],
            _ => i32::MAX,
        };
        intervals.push((start, end, occ.symbol.as_str()));
    }
    if intervals.is_empty() {
        return intervals;
    }
    intervals.sort_by_key(|&(s, _, _)| s);
    let starts: Vec<i32> = intervals.iter().map(|&(s, _, _)| s).collect();
    for (i, slot) in intervals.iter_mut().enumerate() {
        if slot.1 != i32::MAX {
            continue;
        }
        let mut end = i32::MAX;
        for &next_start in starts.iter().skip(i + 1) {
            if next_start > slot.0 {
                end = next_start - 1;
                break;
            }
        }
        slot.1 = end;
    }
    intervals
}

/// Pick the innermost caller interval containing `ref_line` from a
/// pre-built `caller_intervals_for_document` list. Ties broken by
/// the smaller span (i.e. the most deeply nested enclosing function
/// wins).
fn innermost_caller_for_line<'a>(
    ref_line: i32,
    intervals: &[(i32, i32, &'a str)],
) -> Option<&'a str> {
    let mut best: Option<(&'a str, i32)> = None;
    for &(start, end, caller) in intervals {
        if ref_line >= start && ref_line <= end {
            let size = end.saturating_sub(start);
            if best.is_none_or(|(_, best_size)| size < best_size) {
                best = Some((caller, size));
            }
        }
    }
    best.map(|(c, _)| c)
}

/// Roadmap issue #32 v5: does this SCIP symbol's terminal descriptor
/// classify as a `Type`? Parses via the official `scip::symbol`
/// crate so we accept whatever encoding the indexer uses (escaped
/// names, generics, etc.) rather than hand-rolling a `ends_with('#')`
/// heuristic that would miss `Foo#[T].` and friends.
fn is_type_descriptor(symbol: &str) -> bool {
    use scip::types::descriptor::Suffix;
    let Ok(parsed) = scip::symbol::parse_symbol(symbol) else {
        return false;
    };
    let Some(last) = parsed.descriptors.last() else {
        return false;
    };
    matches!(last.suffix.enum_value(), Ok(Suffix::Type))
}

/// Roadmap issue #22: extract a body excerpt from cached file
/// content. Starts at `line_1based` (1-based — matches the
/// editor-gutter convention), takes up to 15 lines, caps the
/// Per iter-191 (closes the scip-python signature gap surfaced
/// by interrogation-037 / -038 + feedback_function_signature_
/// column_empty_py): when SCIP's signature_documentation is
/// empty (universal on scip-python ingest), derive a signature
/// from the first non-blank line of the body excerpt. For
/// Python `def func(args):` lines + JS/TS `function name(args)`
/// lines, the first body line IS the canonical signature.
///
/// Returns the trimmed first non-blank line, stripped of a
/// trailing `:` for Python def lines so the surface looks like
/// a signature rather than the first statement of the body.
/// Empty when body_excerpt has no non-blank lines.
fn derive_signature_from_body(body_excerpt: &str) -> String {
    let first = body_excerpt
        .lines()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("");
    // Strip a trailing colon so Python `def foo(x):` reads as
    // `def foo(x)` — a signature-shaped string the agent can
    // hash for dedup / display alongside language-native
    // signatures.
    first.strip_suffix(':').unwrap_or(first).to_string()
}

/// joined output at 500 bytes. Returns empty string when the
/// content isn't available or `line_1based` is 0 (unknown
/// definition line).
fn extract_body_excerpt(content: Option<&str>, line_1based: u32) -> String {
    const MAX_LINES: usize = 15;
    const MAX_BYTES: usize = 500;

    if line_1based == 0 {
        return String::new();
    }
    let Some(content) = content else {
        return String::new();
    };
    let start = (line_1based as usize).saturating_sub(1);

    let mut out = String::new();
    for (emitted, line) in content.lines().skip(start).enumerate() {
        if emitted >= MAX_LINES {
            break;
        }
        // Pre-check the joined length so we never exceed the cap.
        // The newline separator only applies after the first line;
        // `usize::from(emitted > 0)` cleanly encodes that without
        // tripping the boolean-to-int lint.
        let projected = out.len() + line.len() + usize::from(emitted > 0);
        if projected > MAX_BYTES {
            break;
        }
        if emitted > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

/// Roadmap issue #26: build a `file → last-touched-timestamp` map
/// from `git log` in one pass. Each commit's author-timestamp
/// (ISO-8601) flows through `--format`; the `--name-only` lines
/// emit the changed paths. First time a file appears is the
/// most recent commit that touched it.
///
/// Empty map (returned as default) when the repo isn't a git
/// worktree, `git` isn't on PATH, or the log walk fails for any
/// reason. The caller treats missing entries as empty string,
/// which surfaces as a SQL `f.last_touched = ''` predicate
/// (no false positives in stale-doc joins).
fn git_file_mtimes(root: &Path) -> std::collections::HashMap<String, String> {
    use std::process::Command;

    let output = match Command::new("git")
        .args(["log", "--name-only", "--format=commit %cI"])
        .current_dir(root)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return std::collections::HashMap::new(),
    };
    let Ok(stdout) = String::from_utf8(output.stdout) else {
        return std::collections::HashMap::new();
    };
    parse_git_log_mtimes(&stdout)
}

/// Parse `git log --name-only --format=commit %cI` output into a
/// file-path → most-recent-touch-timestamp map. Factored out from
/// the spawn so the parsing logic gets unit coverage without
/// shelling out.
fn parse_git_log_mtimes(stdout: &str) -> std::collections::HashMap<String, String> {
    let mut out: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut current_ts: Option<String> = None;
    for line in stdout.lines() {
        if let Some(ts) = line.strip_prefix("commit ") {
            current_ts = Some(ts.trim().to_string());
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // First time a path appears = most recent commit touching
        // it. `entry().or_insert` is precisely this idiom.
        if let Some(ts) = current_ts.as_ref() {
            out.entry(trimmed.to_string()).or_insert_with(|| ts.clone());
        }
    }
    out
}

/// Roadmap issue #30: project a repo-relative source file path to its
/// canonical language token. The set tracks the languages SCIP +
/// tree-sitter currently extract; unrecognised extensions get
/// `"unknown"` so the column is always populated (the matcher branches
/// on equality rather than NULL-handling).
fn language_from_file(file: &str) -> &'static str {
    let ext = match file.rsplit_once('.') {
        Some((_, e)) => e,
        None => return "unknown",
    };
    match ext {
        "rs" => "rust",
        "py" => "python",
        "ts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "jsx",
        "cs" => "csharp",
        "dart" => "dart",
        "java" => "java",
        "vue" => "vue",
        _ => "unknown",
    }
}

/// Strip a leading `./` and any leading slash so the path is uniform with
/// what `store` stores for Doc nodes.
fn normalise_relative_path(p: &str) -> String {
    let trimmed = p.trim_start_matches("./").trim_start_matches('/');
    trimmed.to_string()
}

/// Pulls the crate name out of a SCIP symbol string. SCIP symbols look like:
///
/// ```text
/// "rust-analyzer cargo doc-linter 0.1.0 src/parser.rs/parse_doc()."
///  └ scheme       └ pkg.mgr
///                        └ pkg.name (the crate)
///                                   └ pkg.version
///                                          └ descriptors...
/// ```
///
/// SCIP escapes spaces in identifiers with backticks. Ignore that
/// for now — Cargo crate names don't contain spaces in practice.
pub fn extract_crate_name(symbol: &str) -> Option<String> {
    // Use the official scip parser when possible — it correctly handles
    // local symbols, escaped identifiers, etc. Fall back to a naive
    // split-on-space when the parser balks (e.g. forward-incompat schema).
    if let Ok(parsed) = scip::symbol::parse_symbol(symbol) {
        let name = parsed.package.name.trim();
        // scip-java names packages `maven/<group>/<artifact>`; the
        // artifact is the module a Java repo is organised by.
        let name = name
            .strip_prefix("maven/")
            .and_then(|r| r.rsplit('/').next())
            .unwrap_or(name);
        if !name.is_empty() && name != "." {
            return Some(name.to_string());
        }
    }
    // Naive fallback: scheme + pkg.manager + pkg.name + pkg.version + descriptors.
    let mut parts = symbol.split(' ');
    let _scheme = parts.next()?;
    let _mgr = parts.next()?;
    let name = parts.next()?;
    if name == "." || name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Last-resort crate / package name derivation from the file path.
/// Used when SCIP symbol parsing returns no package name — common
/// for non-Rust corpora where `extract_crate_name` short-circuits
/// on local symbols, scip-python's `pkg .` placeholder, etc.
///
/// Resolution (roadmap issue #18):
///   1. `crates/<NAME>/...` — Rust workspace convention. Returns
///      `<NAME>`.
///   2. `.py` / `.ts` / `.tsx` / `.js` / `.jsx` / `.mjs` / `.cjs`
///      files: take the first non-empty directory component as the
///      package name. Pre-#18 these all returned `None`, leaving
///      `Function.crate` empty across the entire Python / TS
///      ingest output — every `MATCH (f) WHERE f.crate <> ''`
///      query missed the whole corpus. Picking the top-level dir
///      (`backend/server.py` → `"backend"`, `frontend/src/app.ts`
///      → `"frontend"`) matches typical project layouts and gives
///      the matcher signal to bucket on; agents that need
///      nested-package granularity can chase the
///      `Function -[:DEFINED_IN_FILE]-> File -[:IN_MODULE]-> Module`
///      chain (Module captures `__init__.py` boundaries from #17).
///   3. Top-level file with no directory (e.g. `setup.py`) — no
///      package name.
fn crate_name_from_file(file: &str) -> Option<String> {
    let mut parts = file.split('/');
    let first = parts.next()?;
    if first == "crates" {
        // Rust workspace path — second component is the crate name.
        let name = parts.next()?;
        if name.is_empty() {
            return None;
        }
        return Some(name.to_string());
    }
    // Non-Rust fallback: language-aware top-directory package
    // inference. Only kicks in for source files in known dynamic /
    // scripting languages, so unfamiliar extensions (.md, .toml,
    // .yaml, ...) still return None.
    let lang = language_from_file(file);
    if matches!(
        lang,
        "python" | "typescript" | "tsx" | "javascript" | "jsx" | "csharp" | "dart" | "vue" | "java"
    ) {
        let second = parts.next()?;
        if second.is_empty() {
            return None; // path was `dir/`, no file after — bail.
        }
        // The first component is the package; bail when the path is
        // a top-level file (`first` was the filename itself, no
        // `second` directory component). `parts.next()` returning
        // None at this point means `first` is the only component.
        return Some(first.to_string());
    }
    None
}

/// Join SCIP `SymbolInformation.documentation` lines into a single string.
/// SCIP emits one entry per markdown paragraph (or per `///` line for
/// some indexers); we join with `\n` to preserve word-boundaries for the
/// downstream entity-mention scanner.
fn join_doc(lines: &[String]) -> String {
    lines.join("\n")
}

/// Convenience for `main.rs::cmd_check` — does the SCIP file at the
/// conventional location exist?
pub fn default_scip_path(root: &Path) -> PathBuf {
    root.join(".doc-lint").join("code.scip")
}

/// Returns true when `scip_path` is older than the newest source
/// file under `root/crates/` or `root/src/`. Roadmap issue #33
/// (v0.3.0): considers `.rs`, `.py`, `.ts` so Python and TypeScript
/// repos see the same staleness signal Rust ones do. Best-effort:
/// any I/O error or missing inputs return `None` (do-nothing rather
/// than spurious diag).
pub fn scip_is_stale(scip_path: &Path, root: &Path) -> Option<u64> {
    let scip_mtime = std::fs::metadata(scip_path).ok()?.modified().ok()?;
    let scan_roots = [root.join("crates"), root.join("src")];
    let mut newest: Option<std::time::SystemTime> = None;
    for r in &scan_roots {
        if !r.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(r).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let ext = entry.path().extension().and_then(|s| s.to_str());
            if !matches!(ext, Some("rs" | "py" | "ts" | "cs" | "dart" | "java")) {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                if let Ok(mt) = meta.modified() {
                    if newest.is_none_or(|n| mt > n) {
                        newest = Some(mt);
                    }
                }
            }
        }
    }
    let newest = newest?;
    if newest > scip_mtime {
        let elapsed = newest.duration_since(scip_mtime).ok()?;
        let days = elapsed.as_secs() / 86400;
        Some(days)
    } else {
        None
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;

    #[test]
    fn lombok_getter_shadowing_a_declared_overload_is_generated() {
        use scip::types::{Document, Occurrence, SymbolInformation};
        let src = "class C {\n    public Money getAmount(final MonetaryCurrency currency) { return null; }\n}";
        let pkg = "semanticdb maven . 1.0 org/x/C#";
        let mut doc = Document::default();
        for (sym, sig) in [
            (
                "getAmount().",
                "public Money getAmount(MonetaryCurrency currency)",
            ),
            (
                "getAmount(+1).",
                "@SuppressWarnings(\"all\") @Generated public BigDecimal getAmount()",
            ),
        ] {
            let mut o = Occurrence::default();
            o.symbol = format!("{pkg}{sym}");
            o.symbol_roles = 1;
            o.range = vec![1, 17, 26]; // both on the declared name
            doc.occurrences.push(o);
            let mut d = Document::default();
            d.text = sig.to_string();
            let mut i = SymbolInformation::default();
            i.symbol = format!("{pkg}{sym}");
            i.signature_documentation = Some(d).into();
            doc.symbols.push(i);
        }
        assert_eq!(demote_generated_java_definitions(&mut doc, src), 1);
        let defined: Vec<&str> = doc
            .occurrences
            .iter()
            .filter(|o| o.symbol_roles & 1 == 1)
            .map(|o| o.symbol.rsplit('#').next().unwrap())
            .collect();
        assert_eq!(defined, ["getAmount()."]);
    }

    /// `@GeneratedValue` (JPA) is not `@Generated`: a hand-written
    /// property-access getter carrying it stays a declared method.
    #[test]
    fn jpa_generated_value_is_not_lombok_generated() {
        use scip::types::{Document, Occurrence, SymbolInformation};
        let src = "class C {\n    @Id @GeneratedValue public Long getId() { return id; }\n}";
        let sym = "semanticdb maven . 1.0 org/x/C#getId().";
        let mut doc = Document::default();
        let mut o = Occurrence::default();
        o.symbol = sym.to_string();
        o.symbol_roles = 1;
        o.range = vec![1, 36, 41];
        doc.occurrences.push(o);
        let mut d = Document::default();
        d.text = "@Id @GeneratedValue(strategy = IDENTITY) public Long getId()".to_string();
        let mut i = SymbolInformation::default();
        i.symbol = sym.to_string();
        i.signature_documentation = Some(d).into();
        doc.symbols.push(i);
        assert_eq!(demote_generated_java_definitions(&mut doc, src), 0);
        assert!(has_generated_annotation("@Generated public int x()"));
        assert!(has_generated_annotation("@lombok.Generated int x()"));
        assert!(has_generated_annotation("@Generated(\"x\") int x()"));
        assert!(!has_generated_annotation("@GeneratedValue int x()"));
    }

    #[test]
    fn java_generated_members_are_demoted_and_declared_ones_kept() {
        use scip::types::{Document, Occurrence};
        let src = [
            "@Getter",                                                        // 0
            "@Setter",                                                        // 1
            "@Accessors(chain = true)",                                       // 2
            "public class JobParameter extends Base {",                       // 3
            "    private Long jobId;",                                        // 4
            "    public static JobParameter getInstance(final Long jobId) {", // 5
            "        return new JobParameter().setJobId(jobId);",             // 6
            "    }",                                                          // 7
            "    public JobParameter(Long id) { this.jobId = id; }",          // 8
            "    String name() { return getJobId().toString(); }",            // 9
            "    @Operation(operationId = \"retrieveAll\")",                  // 10
            "    public String retrieveAll() { return \"\"; }",               // 11
            "    @JsonProperty(\"orphan\") private String orphan;",           // 12
            "    @Path(\"/template\")",                                       // 13
            "    @Operation(description = \"\"\"",                            // 14
            "            Long text.",                                         // 15
            "            \"\"\")",                                            // 16
            "    public String template() { return \"\"; }",                  // 17
            "    @Override",                                                  // 18
            "    // the executor would run on a pool thread",                 // 19
            "    public void run() {}",                                       // 20
            "}",
        ]
        .join("\n");
        let pkg = "semanticdb maven maven/org.x/core 1.0 org/x/JobParameter#";
        let def = |sym: &str, range: Vec<i32>| {
            let mut o = Occurrence::default();
            o.symbol = format!("{pkg}{sym}");
            o.symbol_roles = 1;
            o.range = range;
            o
        };
        let mut doc = Document::default();
        doc.occurrences = vec![
            def("getInstance().", vec![5, 31, 42]),  // declared
            def("setJobId().", vec![6, 35, 43]),     // Lombok: call site
            def("getJobId().", vec![9, 27, 35]),     // Lombok: implicit-this call
            def("`<init>`().", vec![3, 13, 25]),     // generated: class name
            def("`<init>`(+1).", vec![8, 11, 23]),   // declared constructor
            def("setOther().", vec![1, 1, 2, 3]),    // multi-line: annotation
            def("retrieveAll().", vec![10, 30, 41]), // annotation string; declared below
            def("orphan().", vec![12, 19, 25]),      // string, no declaration
            def("template().", vec![13, 12, 20]),    // inside "/template", text block between
            def("run().", vec![19, 26, 29]),         // inside a comment, declared below
        ];
        assert_eq!(demote_generated_java_definitions(&mut doc, &src), 5);
        let still_defined: Vec<&str> = doc
            .occurrences
            .iter()
            .filter(|o| o.symbol_roles & 1 == 1)
            .map(|o| o.symbol.rsplit('#').next().unwrap())
            .collect();
        assert_eq!(
            still_defined,
            [
                "getInstance().",
                "`<init>`(+1).",
                "retrieveAll().",
                "template().",
                "run()."
            ]
        );
    }

    #[test]
    fn extracts_crate_name_from_typical_symbol() {
        // Canonical SCIP-rust symbol: scheme + pkg.mgr + pkg.name + pkg.version + descriptors.
        let sym = "rust-analyzer cargo doc-linter 0.1.0 src/parser.rs/parse_doc().";
        assert_eq!(extract_crate_name(sym).as_deref(), Some("doc-linter"));
    }

    #[test]
    fn extracts_crate_name_with_dotted_descriptors() {
        let sym = "rust-analyzer cargo pricing-core 1.2.3 src/lib.rs/Pricing#compute().";
        assert_eq!(extract_crate_name(sym).as_deref(), Some("pricing-core"));
    }

    #[test]
    fn extract_returns_none_for_local_symbol() {
        // Local symbols use the "local <id>" form — no package.
        let sym = "local 0";
        assert!(extract_crate_name(sym).is_none());
    }

    #[test]
    fn crate_name_from_file_handles_workspace_layout() {
        assert_eq!(
            crate_name_from_file("crates/pricing-core/src/lib.rs").as_deref(),
            Some("pricing-core")
        );
        assert_eq!(crate_name_from_file("docs/foo.md"), None);
        // Roadmap #18: top-level Rust files (no `crates/` prefix)
        // fall through the non-Rust fallback — .rs files stay None
        // because the package boundary is the Cargo.toml dir, not
        // the top-level dir.
        assert_eq!(crate_name_from_file("src/main.rs"), None);
    }

    #[test]
    fn crate_name_from_file_python_picks_top_directory() {
        // Roadmap #18: Python repos pre-#18 had every Function row
        // landing with crate="". Top-level directory wins, matching
        // typical layouts.
        assert_eq!(
            crate_name_from_file("backend/server.py").as_deref(),
            Some("backend")
        );
        assert_eq!(
            crate_name_from_file("tests/test_foo.py").as_deref(),
            Some("tests")
        );
        // Nested dirs still bucket to the top-level package — the
        // Module node (#17) captures __init__.py boundaries for
        // finer-grained navigation.
        assert_eq!(
            crate_name_from_file("backend/api/auth/login.py").as_deref(),
            Some("backend")
        );
    }

    #[test]
    fn crate_name_from_file_typescript_picks_top_directory() {
        assert_eq!(
            crate_name_from_file("frontend/src/app.ts").as_deref(),
            Some("frontend")
        );
        assert_eq!(
            crate_name_from_file("packages/ui/Button.tsx").as_deref(),
            Some("packages")
        );
        assert_eq!(
            crate_name_from_file("apps/web/index.jsx").as_deref(),
            Some("apps")
        );
    }

    #[test]
    fn crate_name_from_file_top_level_script_returns_none() {
        // No directory component → no package name. Top-level
        // scripts like setup.py / build.js bucket to no crate.
        assert_eq!(crate_name_from_file("setup.py"), None);
        assert_eq!(crate_name_from_file("build.js"), None);
    }

    #[test]
    fn language_from_file_maps_extensions() {
        assert_eq!(language_from_file("src/lib.rs"), "rust");
        assert_eq!(language_from_file("backend/server.py"), "python");
        assert_eq!(language_from_file("src/app.ts"), "typescript");
        assert_eq!(language_from_file("components/Button.tsx"), "tsx");
        assert_eq!(language_from_file("scripts/build.js"), "javascript");
        assert_eq!(language_from_file("scripts/build.mjs"), "javascript");
        assert_eq!(language_from_file("Component.jsx"), "jsx");
        assert_eq!(
            language_from_file("Api/Controllers/OutletController.cs"),
            "csharp"
        );
        assert_eq!(language_from_file("lib/main.dart"), "dart");
        assert_eq!(language_from_file("src/main/java/Loan.java"), "java");
        assert_eq!(
            extract_crate_name(
                "semanticdb maven maven/org.apache.fineract/fineract-loan 1.0 org/x/Loans#submit()."
            )
            .as_deref(),
            Some("fineract-loan")
        );
    }

    #[test]
    fn language_from_file_unknown_extension_falls_through() {
        assert_eq!(language_from_file("Makefile"), "unknown");
        assert_eq!(language_from_file("docs/README.md"), "unknown");
        assert_eq!(language_from_file("config.toml"), "unknown");
        // Path with no extension at all — `rsplit_once` returns None.
        assert_eq!(language_from_file("noextension"), "unknown");
    }

    #[test]
    fn extract_body_excerpt_handles_missing_inputs() {
        // Roadmap #22: every non-fatal failure path returns empty.
        assert_eq!(extract_body_excerpt(None, 5), "");
        assert_eq!(
            extract_body_excerpt(Some("fn main() {}\n"), 0),
            "",
            "line=0 means unknown definition line — bail"
        );
    }

    #[test]
    fn extract_body_excerpt_starts_at_1_based_line() {
        let src = "fn one() {}\nfn two() {\n    println!(\"hi\");\n}\nfn three() {}\n";
        // line=2 means start at "fn two() {"
        let out = extract_body_excerpt(Some(src), 2);
        assert!(out.starts_with("fn two()"), "got: {out:?}");
        assert!(out.contains("println!"));
    }

    #[test]
    fn extract_body_excerpt_caps_at_15_lines() {
        let src = (1..=30)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = extract_body_excerpt(Some(&src), 1);
        let count = out.lines().count();
        assert_eq!(count, 15, "MAX_LINES cap");
        assert!(out.starts_with("line1"));
        assert!(out.ends_with("line15"));
    }

    #[test]
    fn extract_body_excerpt_caps_at_500_bytes() {
        let long = "x".repeat(600);
        let out = extract_body_excerpt(Some(&long), 1);
        assert!(out.len() <= 500, "MAX_BYTES cap; got {} bytes", out.len());
    }

    #[test]
    fn parse_git_log_mtimes_first_touch_wins() {
        // Roadmap #26: simulate `git log --name-only --format='commit %cI'`
        // output. `src/lib.rs` appears in both commits; the first
        // (most recent) timestamp must win.
        let stdout = "\
commit 2026-05-15T10:00:00+00:00

src/lib.rs
docs/foo.md

commit 2026-05-10T08:00:00+00:00

src/lib.rs
src/parser.rs
";
        let map = parse_git_log_mtimes(stdout);
        assert_eq!(
            map.get("src/lib.rs").map(String::as_str),
            Some("2026-05-15T10:00:00+00:00"),
            "most recent commit wins"
        );
        assert_eq!(
            map.get("docs/foo.md").map(String::as_str),
            Some("2026-05-15T10:00:00+00:00")
        );
        assert_eq!(
            map.get("src/parser.rs").map(String::as_str),
            Some("2026-05-10T08:00:00+00:00")
        );
    }

    #[test]
    fn parse_git_log_mtimes_handles_empty_stdout() {
        // Non-git repo / missing `git`: empty stdout → empty map.
        assert!(parse_git_log_mtimes("").is_empty());
    }

    /// #263: static methods and constructors are Function rows; an
    /// implicit default constructor (definition on the class name, no
    /// declaration) is flagged generated.
    #[test]
    fn parse_scip_keeps_static_methods_and_constructors() {
        use scip::types::{Document, Occurrence, SymbolInformation};
        let dir = std::env::temp_dir().join(format!(
            "doc-linter-scip-static-ctor-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/Foo.java"),
            "class Foo {\n    static Foo make() { return new Foo(); }\n}\n\
             class Bar {\n    Bar(int x) {}\n}\n",
        )
        .unwrap();
        let pkg = "semanticdb maven . 1.0 org/x/";
        let mut doc = Document::default();
        doc.relative_path = "src/Foo.java".to_string();
        doc.language = "java".to_string();
        for (sym, kind, range) in [
            ("Foo#make().", ScipKind::StaticMethod, vec![1, 15, 19]),
            ("Foo#`<init>`().", ScipKind::Constructor, vec![0, 6, 9]),
            ("Bar#`<init>`().", ScipKind::Constructor, vec![4, 4, 7]),
        ] {
            let mut i = SymbolInformation::default();
            i.symbol = format!("{pkg}{sym}");
            i.kind = kind.into();
            doc.symbols.push(i);
            let mut o = Occurrence::default();
            o.symbol = format!("{pkg}{sym}");
            o.symbol_roles = 1;
            o.range = range;
            doc.occurrences.push(o);
        }
        let mut index = Index::default();
        index.documents.push(doc);
        let path = dir.join("code.scip");
        std::fs::write(&path, index.write_to_bytes().unwrap()).unwrap();

        let facts = parse_scip(&path, &dir).unwrap();
        let mut got: Vec<(&str, &str, bool)> = facts
            .functions
            .iter()
            .map(|f| {
                let name = f.symbol.strip_prefix(pkg).unwrap();
                (name, f.kind.as_str(), f.generated)
            })
            .collect();
        got.sort_unstable();
        assert_eq!(
            got,
            [
                ("Bar#`<init>`().", "method", false),
                ("Foo#`<init>`().", "method", true),
                ("Foo#make().", "method", false),
            ]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// #268: a call through an interface method also reaches every
    /// in-index implementation (a `dispatch` CALLS edge); overriding a
    /// method from outside the index (`Object#toString`) adds nothing.
    #[test]
    fn parse_scip_adds_dispatch_calls_to_implementations() {
        use scip::types::{Document, Occurrence, Relationship, SymbolInformation};
        let dir = std::env::temp_dir().join(format!("doc-linter-dispatch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pkg = "semanticdb maven . 1.0 org/x/";
        let iface = format!("{pkg}Handler#process().");
        let imp = format!("{pkg}LoanHandler#process().");
        let to_string = format!("{pkg}LoanHandler#toString().");
        let caller = format!("{pkg}Runner#run().");
        let mut doc = Document::default();
        doc.relative_path = "src/X.java".to_string();
        let mut def = |sym: &str, kind: ScipKind, line: i32, rel: Option<&str>| {
            let mut i = SymbolInformation::default();
            i.symbol = sym.to_string();
            i.kind = kind.into();
            if let Some(r) = rel {
                let mut rl = Relationship::default();
                rl.symbol = r.to_string();
                rl.is_implementation = true;
                i.relationships.push(rl);
            }
            doc.symbols.push(i);
            let mut o = Occurrence::default();
            o.symbol = sym.to_string();
            o.symbol_roles = 1;
            o.range = vec![line, 0, 1];
            o.enclosing_range = vec![line, 0, line, 80];
            doc.occurrences.push(o);
        };
        def(&iface, ScipKind::AbstractMethod, 0, None);
        def(&imp, ScipKind::Method, 1, Some(&iface));
        def(
            &to_string,
            ScipKind::Method,
            2,
            Some("semanticdb maven jdk 11 java/lang/Object#toString()."),
        );
        def(&caller, ScipKind::Method, 3, None);
        for callee in [
            iface.clone(),
            "semanticdb maven jdk 11 java/lang/Object#toString().".to_string(),
        ] {
            let mut o = Occurrence::default();
            o.symbol = callee;
            o.range = vec![3, 20, 27];
            doc.occurrences.push(o);
        }
        let mut index = Index::default();
        index.documents.push(doc);
        let path = dir.join("code.scip");
        std::fs::write(&path, index.write_to_bytes().unwrap()).unwrap();

        let facts = parse_scip(&path, &dir).unwrap();
        let mut got: Vec<(&str, bool)> = facts
            .calls
            .iter()
            .map(|c| (c.callee_symbol.rsplit('/').next().unwrap(), c.dispatch))
            .collect();
        got.sort_unstable();
        assert_eq!(
            got,
            [
                ("Handler#process().", false),
                ("LoanHandler#process().", true),
                ("Object#toString().", false),
            ]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// #227: scip-python never sets the Import role and names modules
    /// `` `pkg.hparams` `` (no `/`, no `.py`). Its import statements still
    /// become IMPORTS facts: the reference on a `from` / `import` line,
    /// resolved through the module each document defines. A use of the
    /// same symbol elsewhere in the file is not an import.
    #[test]
    fn parse_scip_derives_python_imports_without_import_role() {
        use scip::types::{Document, Occurrence};
        let dir =
            std::env::temp_dir().join(format!("doc-linter-py-imports-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("pkg")).unwrap();
        std::fs::write(dir.join("pkg/hparams.py"), "hparams = {}\n").unwrap();
        std::fs::write(
            dir.join("pkg/train.py"),
            "import os\nfrom pkg.hparams import hparams\n\nlr = hparams['lr']\n",
        )
        .unwrap();
        let sym = |s: &str| format!("scip-python python demo 0.1 {s}");
        let occ = |s: &str, line: i32, roles: i32| {
            let mut o = Occurrence::default();
            o.symbol = sym(s);
            o.range = vec![line, 0, 5];
            o.symbol_roles = roles;
            o
        };
        let mut hp = Document::default();
        hp.relative_path = "pkg/hparams.py".to_string();
        hp.occurrences.push(occ("`pkg.hparams`/hparams.", 0, 1));
        let mut train = Document::default();
        train.relative_path = "pkg/train.py".to_string();
        train.occurrences.push(occ("`pkg.train`/lr.", 3, 1));
        train.occurrences.push(occ("`os`/__init__:", 0, 8));
        train.occurrences.push(occ("`pkg.hparams`/hparams.", 1, 8)); // the import
        train.occurrences.push(occ("`pkg.hparams`/hparams.", 3, 8)); // a use
        let mut index = Index::default();
        index.documents = vec![hp, train];
        let path = dir.join("code.scip");
        std::fs::write(&path, index.write_to_bytes().unwrap()).unwrap();

        let facts = parse_scip(&path, &dir).unwrap();
        let got: Vec<(&str, &str)> = facts
            .imports
            .iter()
            .map(|i| (i.importer_file.as_str(), i.imported_file.as_str()))
            .collect();
        assert_eq!(got, [("pkg/train.py", "pkg/hparams.py")]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn function_kind_round_trips_via_str() {
        for k in [
            FunctionKind::Function,
            FunctionKind::Method,
            FunctionKind::Struct,
            FunctionKind::Enum,
            FunctionKind::Trait,
            FunctionKind::Module,
            FunctionKind::TypeAlias,
            FunctionKind::Other,
        ] {
            // Just exercise the as_str path so a future enum addition
            // forces an update.
            let _ = k.as_str();
        }
    }

    #[test]
    fn doc_join_preserves_paragraph_boundaries() {
        let lines = vec!["First line.".to_string(), "Second line.".to_string()];
        let joined = join_doc(&lines);
        assert_eq!(joined, "First line.\nSecond line.");
    }

    #[test]
    fn parse_scip_rejects_empty_file() {
        let dir =
            std::env::temp_dir().join(format!("doc-linter-scip-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("empty.scip");
        std::fs::write(&f, b"").unwrap();
        let err = parse_scip(&f, &dir).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    /// End-to-end: build a SCIP `Index` in memory using the protobuf
    /// types, serialize to bytes, write to disk, then `parse_scip`. Asserts
    /// we round-trip a known function symbol with its doc-comment.
    #[test]
    fn parse_scip_round_trips_a_synthetic_index() {
        use scip::types::{Document, Occurrence, SymbolInformation};

        let dir = std::env::temp_dir().join(format!(
            "doc-linter-scip-rt-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut index = Index::default();
        let mut doc = Document::default();
        doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();
        doc.language = "rust".to_string();

        // Symbol: a function named `compute` in pricing-core.
        let symbol_str = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/compute().";
        let mut sym = SymbolInformation::default();
        sym.symbol = symbol_str.to_string();
        sym.documentation = vec![
            "Computes pricing for an order.".to_string(),
            "Returns a [`PricingRule`] match.".to_string(),
        ];
        sym.kind = scip::types::symbol_information::Kind::Function.into();
        doc.symbols.push(sym);

        // Definition occurrence at line 0, col 4 (0-based).
        let mut occ = Occurrence::default();
        occ.symbol = symbol_str.to_string();
        occ.range = vec![0, 4, 0, 11];
        occ.symbol_roles = 1; // Definition
        doc.occurrences.push(occ);

        index.documents.push(doc);

        let bytes = index.write_to_bytes().expect("encode");
        let path = dir.join("code.scip");
        std::fs::write(&path, &bytes).unwrap();

        let facts = parse_scip(&path, &dir).unwrap();
        assert_eq!(facts.functions.len(), 1, "exactly one function");
        let f = &facts.functions[0];
        assert_eq!(f.symbol, symbol_str);
        assert_eq!(f.crate_name, "pricing-core");
        assert_eq!(f.file, "crates/pricing-core/src/lib.rs");
        assert_eq!(f.line, 1, "line is 1-based");
        assert!(
            f.doc_comment.contains("Computes pricing"),
            "doc-comment lines joined into doc_comment: {:?}",
            f.doc_comment
        );
        assert_eq!(f.kind, FunctionKind::Function);
        // Roadmap #30: language derived from extension.
        assert_eq!(f.language, "rust");
    }

    /// scip-python 0.6.x regression: every symbol arrives with
    /// `UnspecifiedKind` (the protobuf default). Before the
    /// descriptor-suffix fallback, the entire file was silently dropped.
    /// Now we ingest each symbol with its inferred kind and bump
    /// `inferred_count`.
    #[test]
    fn parse_scip_falls_back_when_kind_is_unspecified() {
        use scip::types::{Document, Occurrence, SymbolInformation};

        let dir = std::env::temp_dir().join(format!(
            "doc-linter-scip-unspecified-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut index = Index::default();
        let mut doc = Document::default();
        doc.relative_path = "src/pkg/auth.py".to_string();
        doc.language = "python".to_string();

        // Three symbols mimicking scip-python output: a top-level function,
        // a class, and a method. None set `kind` — the protobuf default
        // (UnspecifiedKind) stays in place.
        let fn_sym = "scip-python python pkg . src/pkg/auth.py/verify_token().";
        let class_sym = "scip-python python pkg . src/pkg/auth.py/TokenStore#";
        let method_sym = "scip-python python pkg . src/pkg/auth.py/TokenStore#refresh().";

        for s in [fn_sym, class_sym, method_sym] {
            let mut sym = SymbolInformation::default();
            sym.symbol = s.to_string();
            // Deliberately leave `sym.kind` at the default — that's the bug
            // scenario this test exercises.
            doc.symbols.push(sym);

            let mut occ = Occurrence::default();
            occ.symbol = s.to_string();
            occ.range = vec![0, 0, 0, 1];
            occ.symbol_roles = 1;
            doc.occurrences.push(occ);
        }
        index.documents.push(doc);

        let bytes = index.write_to_bytes().expect("encode");
        let path = dir.join("code.scip");
        std::fs::write(&path, &bytes).unwrap();

        let facts = parse_scip(&path, &dir).unwrap();
        assert_eq!(facts.functions.len(), 3, "all three symbols ingested");
        assert_eq!(facts.inferred_count, 3, "all three reached fallback");

        let mut by_kind: Vec<FunctionKind> = facts.functions.iter().map(|f| f.kind).collect();
        by_kind.sort_by_key(|k| k.as_str());
        assert_eq!(
            by_kind,
            vec![
                FunctionKind::Function,
                FunctionKind::Method,
                FunctionKind::Struct
            ],
            "descriptor suffix infers Function (().), Method (Type#name().), Struct (Type#)"
        );
    }

    /// Roadmap issue #9: parse_scip emits one CallFact per resolved
    /// reference occurrence. Build a synthetic SCIP doc with two
    /// function defs (caller body lines 0–9, callee def line 12),
    /// drop a Reference occurrence of the callee inside the caller's
    /// body, and assert the resulting fact pins the right caller →
    /// callee pair at the right file + line.
    #[test]
    fn parse_scip_emits_calls_for_in_function_references() {
        use scip::types::{Document, Occurrence, SymbolInformation};

        let dir = std::env::temp_dir().join(format!(
            "doc-linter-scip-calls-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut index = Index::default();
        let mut doc = Document::default();
        doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();
        doc.language = "rust".to_string();

        let caller = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/orchestrate().";
        let callee = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/compute().";

        for s in [caller, callee] {
            let mut sym = SymbolInformation::default();
            sym.symbol = s.to_string();
            sym.kind = scip::types::symbol_information::Kind::Function.into();
            doc.symbols.push(sym);
        }

        // Caller def: line 0, body span [0, 9].
        let mut caller_def = Occurrence::default();
        caller_def.symbol = caller.to_string();
        caller_def.range = vec![0, 4, 0, 15];
        caller_def.enclosing_range = vec![0, 0, 9, 0];
        caller_def.symbol_roles = 1;
        doc.occurrences.push(caller_def);

        // Callee def: line 12, body span [12, 14].
        let mut callee_def = Occurrence::default();
        callee_def.symbol = callee.to_string();
        callee_def.range = vec![12, 4, 12, 11];
        callee_def.enclosing_range = vec![12, 0, 14, 0];
        callee_def.symbol_roles = 1;
        doc.occurrences.push(callee_def);

        // Reference: line 5, inside caller's body. symbol_roles = 0
        // (pure reference; no Definition bit).
        let mut call_occ = Occurrence::default();
        call_occ.symbol = callee.to_string();
        call_occ.range = vec![5, 8, 5, 15];
        doc.occurrences.push(call_occ);

        index.documents.push(doc);
        let bytes = index.write_to_bytes().expect("encode");
        let path = dir.join("code.scip");
        std::fs::write(&path, &bytes).unwrap();

        let facts = parse_scip(&path, &dir).unwrap();
        assert_eq!(facts.calls.len(), 1, "exactly one CALLS fact");
        let c = &facts.calls[0];
        assert_eq!(c.caller_symbol, caller);
        assert_eq!(c.callee_symbol, callee);
        assert_eq!(c.source_file, "crates/pricing-core/src/lib.rs");
        assert_eq!(c.source_line, 6, "line is 1-based, ref was at 0-based 5");
    }

    /// scip-typescript's `export const f = (…) => …` / `= function …` /
    /// `= async (…) => …` are terms whose hover type is a function; a
    /// plain constant and a field are not. Hover texts are from a live
    /// scip-typescript run.
    #[test]
    fn ts_function_valued_variables_are_functions() {
        let sym = |s: &str, hover: &str| {
            let mut sym = scip::types::SymbolInformation::default();
            sym.symbol = s.to_string();
            sym.documentation = vec![format!("```ts\n{hover}\n```")];
            sym
        };
        for (s, hover) in [
            (
                "scip-typescript npm x HEAD `api.ts`/mark.",
                "var mark: (p: Product) => number",
            ),
            (
                "scip-typescript npm x HEAD `api.ts`/legacy.",
                "var legacy: (p: Product) => number",
            ),
            (
                "scip-typescript npm x HEAD `api.ts`/asyncArrow.",
                "var asyncArrow: (x: number) => Promise<number>",
            ),
            (
                "scip-typescript npm x HEAD `api.ts`/id.",
                "const id: <T>(x: T) => T",
            ),
        ] {
            assert!(
                matches!(resolve_symbol_kind(&sym(s, hover)), FunctionKind::Function),
                "{s}"
            );
        }
        for (s, hover) in [
            (
                "scip-typescript npm x HEAD `api.ts`/notAFunction.",
                "var notAFunction: 42",
            ),
            (
                "scip-typescript npm x HEAD `api.ts`/Product#onClick.",
                "(property) onClick: () => void",
            ),
            ("local 1", "(parameter) p: Product"),
        ] {
            assert!(
                matches!(resolve_symbol_kind(&sym(s, hover)), FunctionKind::Other),
                "{s}"
            );
        }
    }

    /// Legacy gap 7: fields / properties are `Type#name.` in every
    /// indexer sampled live (scip-dotnet, scip-typescript, scip_dart).
    #[test]
    fn field_parts_recognises_type_members_only() {
        for (symbol, owner, name) in [
            (
                "scip-dotnet nuget . . Lib/Product#IsFreeProduct.",
                "scip-dotnet nuget . . Lib/Product#",
                "IsFreeProduct",
            ),
            (
                "scip-typescript npm x HEAD `api.ts`/Product#isFreeProduct.",
                "scip-typescript npm x HEAD `api.ts`/Product#",
                "isFreeProduct",
            ),
            (
                "scip-dart pub fa_app . lib/`pricing.dart`/Product#price.",
                "scip-dart pub fa_app . lib/`pricing.dart`/Product#",
                "price",
            ),
            (
                "rust-analyzer cargo demo 0.1.0 config/LintConfig#skip_dirs.",
                "rust-analyzer cargo demo 0.1.0 config/LintConfig#",
                "skip_dirs",
            ),
        ] {
            assert_eq!(
                field_parts(symbol),
                Some((owner.to_string(), name.to_string())),
                "{symbol}"
            );
        }
        for not_a_field in [
            "scip-dotnet nuget . . Lib/Pricing#Base().",
            "scip-dotnet nuget . . Lib/Product#",
            "scip-dotnet nuget . . Lib/Pricing#Base().(p)",
            "scip-typescript npm x HEAD `api.ts`/mark.",
            "local 3",
        ] {
            assert_eq!(field_parts(not_a_field), None, "{not_a_field}");
        }
    }

    /// Reads and writes of a field, including from a closure, become one
    /// REFERENCES fact per (function, field) at the first line.
    #[test]
    fn field_references_are_attributed_and_deduped() {
        use scip::types::{Document, Occurrence, SymbolInformation};

        let base = "scip-dotnet nuget . . Lib/Pricing#Base().";
        let field = "scip-dotnet nuget . . Lib/Product#IsFreeProduct.";
        let mut doc = Document::default();
        for s in [base, "local 1", field] {
            let mut sym = SymbolInformation::default();
            sym.symbol = s.to_string();
            doc.symbols.push(sym);
        }
        for (s, line, span) in [(base, 0, [0, 10]), ("local 1", 3, [3, 5])] {
            let mut def = Occurrence::default();
            def.symbol = s.to_string();
            def.range = vec![line, 2, line, 8];
            def.enclosing_range = vec![span[0], 0, span[1], 0];
            def.symbol_roles = 1;
            doc.occurrences.push(def);
        }
        let mut field_def = Occurrence::default();
        field_def.symbol = field.to_string();
        field_def.range = vec![20, 2, 20, 9];
        field_def.symbol_roles = 1;
        doc.occurrences.push(field_def);
        for line in [2, 4, 6] {
            let mut r = Occurrence::default();
            r.symbol = field.to_string();
            r.range = vec![line, 10, line, 23];
            doc.occurrences.push(r);
        }
        let mut refs = Vec::new();
        derive_references_for_document(&doc, "Lib/Pricing.cs", &mut refs);
        assert_eq!(refs.len(), 1, "{refs:?}");
        assert_eq!(refs[0].function_symbol, base);
        assert_eq!(refs[0].field_symbol, field);
        assert_eq!(refs[0].source_line, 3);
    }

    /// Legacy gap 9: a call inside a Dart closure (`local 3`) used to be
    /// attributed to the closure symbol, which has no file; it belongs to
    /// the enclosing method.
    #[test]
    fn calls_inside_a_closure_belong_to_the_enclosing_function() {
        use scip::types::{Document, Occurrence, SymbolInformation};

        let outer = "scip-dart pub fa_app 1.0.0 lib/`pricing.dart`/Pricing#apply().";
        let callee = "scip-dart pub fa_app 1.0.0 lib/`pricing.dart`/discount().";
        let mut doc = Document::default();
        for (s, kind) in [
            (outer, scip::types::symbol_information::Kind::Method),
            ("local 3", scip::types::symbol_information::Kind::Function),
            (callee, scip::types::symbol_information::Kind::Function),
        ] {
            let mut sym = SymbolInformation::default();
            sym.symbol = s.to_string();
            sym.kind = kind.into();
            doc.symbols.push(sym);
        }
        for (s, line, span) in [
            (outer, 0, [0, 10]),
            ("local 3", 2, [2, 4]),
            (callee, 20, [20, 22]),
        ] {
            let mut def = Occurrence::default();
            def.symbol = s.to_string();
            def.range = vec![line, 2, line, 8];
            def.enclosing_range = vec![span[0], 0, span[1], 0];
            def.symbol_roles = 1;
            doc.occurrences.push(def);
        }
        let mut call = Occurrence::default();
        call.symbol = callee.to_string();
        call.range = vec![3, 10, 3, 18];
        doc.occurrences.push(call);

        let mut calls = Vec::new();
        derive_calls_for_document(
            &doc,
            "fa_app/lib/pricing.dart",
            &std::collections::HashSet::new(),
            &mut calls,
        );
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].caller_symbol, outer);
    }

    /// A call to a second or later overload (`foo(+1).`) is a call; a
    /// parameter (`foo().(x)`) is not.
    #[test]
    fn derive_calls_counts_overloads_not_parameters() {
        use scip::types::{Document, Occurrence, SymbolInformation};
        let caller = "semanticdb maven . 1.0 org/x/Loans#run().";
        let overload = "semanticdb maven . 1.0 org/x/Charge#getAmount(+1).";
        let param = "semanticdb maven . 1.0 org/x/Charge#getAmount().(currency)";
        let mut doc = Document::default();
        let mut sym = SymbolInformation::default();
        sym.symbol = caller.to_string();
        sym.kind = scip::types::symbol_information::Kind::Method.into();
        doc.symbols.push(sym);
        let mut def = Occurrence::default();
        def.symbol = caller.to_string();
        def.range = vec![0, 4, 0, 7];
        def.enclosing_range = vec![0, 0, 9, 0];
        def.symbol_roles = 1;
        doc.occurrences.push(def);
        for (s, line) in [(overload, 3), (param, 4)] {
            let mut r = Occurrence::default();
            r.symbol = s.to_string();
            r.range = vec![line, 10, line, 19];
            doc.occurrences.push(r);
        }
        let mut calls = Vec::new();
        derive_calls_for_document(
            &doc,
            "Loans.java",
            &std::collections::HashSet::new(),
            &mut calls,
        );
        let callees: Vec<&str> = calls.iter().map(|c| c.callee_symbol.as_str()).collect();
        assert_eq!(callees, [overload]);
    }

    /// Reference occurrences whose symbol doesn't end with `().` are
    /// not function calls — type references, field accesses, etc.
    /// They must not show up in `facts.calls`.
    #[test]
    fn parse_scip_skips_non_function_references() {
        use scip::types::{Document, Occurrence, SymbolInformation};

        let dir = std::env::temp_dir().join(format!(
            "doc-linter-scip-noncalls-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut index = Index::default();
        let mut doc = Document::default();
        doc.relative_path = "src/lib.rs".to_string();

        let caller = "rust-analyzer cargo demo 0.1.0 src/lib.rs/run().";
        let type_sym = "rust-analyzer cargo demo 0.1.0 src/lib.rs/Config#";

        let mut sym = SymbolInformation::default();
        sym.symbol = caller.to_string();
        sym.kind = scip::types::symbol_information::Kind::Function.into();
        doc.symbols.push(sym);

        let mut caller_def = Occurrence::default();
        caller_def.symbol = caller.to_string();
        caller_def.range = vec![0, 4, 0, 7];
        caller_def.enclosing_range = vec![0, 0, 5, 0];
        caller_def.symbol_roles = 1;
        doc.occurrences.push(caller_def);

        // Reference to a type — descriptor ends with `#`, not `().`.
        let mut type_ref = Occurrence::default();
        type_ref.symbol = type_sym.to_string();
        type_ref.range = vec![2, 8, 2, 14];
        doc.occurrences.push(type_ref);

        index.documents.push(doc);
        let bytes = index.write_to_bytes().expect("encode");
        let path = dir.join("code.scip");
        std::fs::write(&path, &bytes).unwrap();

        let facts = parse_scip(&path, &dir).unwrap();
        assert_eq!(
            facts.calls.len(),
            0,
            "type references are not function calls"
        );
    }

    /// Roadmap issue #32 v5 (v0.4.0): a SCIP reference occurrence
    /// whose descriptor terminal is a `Type` (suffix `#`) and whose
    /// line falls inside a function body resolves to a single
    /// `UsesTypeFact`. The companion `parse_scip_skips_non_function_references`
    /// test already proves the same occurrence does NOT pollute
    /// `facts.calls` — this is the symmetric assertion that it DOES
    /// land in `facts.uses_types`.
    #[test]
    fn parse_scip_emits_uses_type_for_type_suffix_references() {
        use scip::types::{Document, Occurrence, SymbolInformation};

        let dir = std::env::temp_dir().join(format!(
            "doc-linter-scip-usestype-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut index = Index::default();
        let mut doc = Document::default();
        doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();
        doc.language = "rust".to_string();

        let caller = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/run().";
        let type_sym = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/Config#";

        let mut sym_fn = SymbolInformation::default();
        sym_fn.symbol = caller.to_string();
        sym_fn.kind = scip::types::symbol_information::Kind::Function.into();
        doc.symbols.push(sym_fn);

        let mut sym_ty = SymbolInformation::default();
        sym_ty.symbol = type_sym.to_string();
        sym_ty.kind = scip::types::symbol_information::Kind::Struct.into();
        doc.symbols.push(sym_ty);

        let mut caller_def = Occurrence::default();
        caller_def.symbol = caller.to_string();
        caller_def.range = vec![0, 4, 0, 7];
        caller_def.enclosing_range = vec![0, 0, 5, 0];
        caller_def.symbol_roles = 1;
        doc.occurrences.push(caller_def);

        // Two reference occurrences to the same type inside the body —
        // the dedupe in derive_uses_type_for_document should collapse
        // them to one fact.
        let mut type_ref_a = Occurrence::default();
        type_ref_a.symbol = type_sym.to_string();
        type_ref_a.range = vec![2, 8, 2, 14];
        doc.occurrences.push(type_ref_a);

        let mut type_ref_b = Occurrence::default();
        type_ref_b.symbol = type_sym.to_string();
        type_ref_b.range = vec![3, 12, 3, 18];
        doc.occurrences.push(type_ref_b);

        index.documents.push(doc);
        let bytes = index.write_to_bytes().expect("encode");
        let path = dir.join("code.scip");
        std::fs::write(&path, &bytes).unwrap();

        let facts = parse_scip(&path, &dir).unwrap();
        assert_eq!(
            facts.uses_types.len(),
            1,
            "two refs to the same type collapse to one UsesTypeFact: {:?}",
            facts.uses_types
        );
        let u = &facts.uses_types[0];
        assert_eq!(u.function_symbol, caller);
        assert_eq!(u.type_symbol, type_sym);
        // And the inverse: a type reference must not pollute CALLS —
        // confirms the two derivations stay disjoint.
        assert!(
            facts.calls.is_empty(),
            "type reference must not become a CallFact: {:?}",
            facts.calls
        );
    }

    /// References that fall outside every caller interval (top-of-file
    /// use statements, line 0 module-level decls) have no enclosing
    /// function — they should NOT produce USES_TYPE facts.
    #[test]
    fn parse_scip_skips_uses_type_outside_function_bodies() {
        use scip::types::{Document, Occurrence, SymbolInformation};

        let dir = std::env::temp_dir().join(format!(
            "doc-linter-scip-usestype-nocaller-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut index = Index::default();
        let mut doc = Document::default();
        doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();
        doc.language = "rust".to_string();

        let caller = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/run().";
        let type_sym = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/Config#";

        let mut sym_fn = SymbolInformation::default();
        sym_fn.symbol = caller.to_string();
        sym_fn.kind = scip::types::symbol_information::Kind::Function.into();
        doc.symbols.push(sym_fn);

        // Caller body spans lines 10..20 (well below the type ref).
        let mut caller_def = Occurrence::default();
        caller_def.symbol = caller.to_string();
        caller_def.range = vec![10, 4, 10, 7];
        caller_def.enclosing_range = vec![10, 0, 20, 0];
        caller_def.symbol_roles = 1;
        doc.occurrences.push(caller_def);

        // Top-of-file type reference (line 0) — outside any function.
        let mut type_ref = Occurrence::default();
        type_ref.symbol = type_sym.to_string();
        type_ref.range = vec![0, 4, 0, 10];
        doc.occurrences.push(type_ref);

        index.documents.push(doc);
        let bytes = index.write_to_bytes().expect("encode");
        let path = dir.join("code.scip");
        std::fs::write(&path, &bytes).unwrap();

        let facts = parse_scip(&path, &dir).unwrap();
        assert!(
            facts.uses_types.is_empty(),
            "type refs outside any caller interval must not emit USES_TYPE: {:?}",
            facts.uses_types
        );
    }

    /// Roadmap issue #32 v6 (v0.4.0): a SCIP `SymbolInformation`
    /// for a type-kind symbol with one `is_implementation: true`
    /// relationship yields a single `ImplementsFact` pointing at the
    /// related symbol. Non-type subjects (functions, methods) must
    /// not contribute; non-implementation flags (is_reference,
    /// is_type_definition) on the same relationship must not
    /// contribute either.
    #[test]
    fn parse_scip_emits_implements_for_type_with_is_implementation_relationship() {
        use scip::types::{Document, Occurrence, Relationship, SymbolInformation};

        let dir = std::env::temp_dir().join(format!(
            "doc-linter-scip-implements-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut index = Index::default();
        let mut doc = Document::default();
        doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();
        doc.language = "rust".to_string();

        let struct_sym = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/PricingRule#";
        let trait_sym = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/Validate#";

        // The implementing struct, with one is_implementation relationship
        // to the trait. Also includes a noise relationship (is_reference
        // only) to itself — must NOT produce a second fact.
        let mut sym_struct = SymbolInformation::default();
        sym_struct.symbol = struct_sym.to_string();
        sym_struct.kind = scip::types::symbol_information::Kind::Struct.into();
        let mut rel_impl = Relationship::default();
        rel_impl.symbol = trait_sym.to_string();
        rel_impl.is_implementation = true;
        sym_struct.relationships.push(rel_impl);
        let mut rel_ref = Relationship::default();
        rel_ref.symbol = struct_sym.to_string();
        rel_ref.is_reference = true;
        sym_struct.relationships.push(rel_ref);
        doc.symbols.push(sym_struct);

        // The trait itself — just so the Type table has both endpoints
        // when this is round-tripped through the store (the parser doesn't
        // need it but the integration test will).
        let mut sym_trait = SymbolInformation::default();
        sym_trait.symbol = trait_sym.to_string();
        sym_trait.kind = scip::types::symbol_information::Kind::Trait.into();
        doc.symbols.push(sym_trait);

        // A method on the struct — must NOT emit an IMPLEMENTS fact even
        // if its symbol carries an is_implementation flag, because the
        // subject's kind is Method, not Type.
        let method_sym =
            "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/PricingRule/validate().";
        let mut sym_method = SymbolInformation::default();
        sym_method.symbol = method_sym.to_string();
        sym_method.kind = scip::types::symbol_information::Kind::Method.into();
        let mut rel_method = Relationship::default();
        rel_method.symbol = trait_sym.to_string();
        rel_method.is_implementation = true;
        sym_method.relationships.push(rel_method);
        doc.symbols.push(sym_method);

        // Definition occurrences just so the symbols round-trip cleanly.
        for (s, line) in [(struct_sym, 0), (trait_sym, 1), (method_sym, 3)] {
            let mut occ = Occurrence::default();
            occ.symbol = s.to_string();
            occ.range = vec![line, 0, line, 10];
            occ.symbol_roles = 1;
            doc.occurrences.push(occ);
        }

        index.documents.push(doc);
        let bytes = index.write_to_bytes().expect("encode");
        let path = dir.join("code.scip");
        std::fs::write(&path, &bytes).unwrap();

        let facts = parse_scip(&path, &dir).unwrap();
        assert_eq!(
            facts.implements.len(),
            1,
            "exactly one IMPLEMENTS fact (method subject + is_reference noise filtered out): {:?}",
            facts.implements
        );
        let f = &facts.implements[0];
        assert_eq!(f.from_type, struct_sym);
        assert_eq!(f.to_type, trait_sym);
    }

    /// Repeat `is_implementation` entries on the same (from, to) pair
    /// (e.g. when the indexer emits the same impl block twice across
    /// re-exports) collapse to one fact via the per-document dedupe.
    #[test]
    fn parse_scip_dedupes_implements_per_from_to_pair() {
        use scip::types::{Document, Occurrence, Relationship, SymbolInformation};

        let dir = std::env::temp_dir().join(format!(
            "doc-linter-scip-implements-dedupe-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut index = Index::default();
        let mut doc = Document::default();
        doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();

        let struct_sym = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/Rule#";
        let trait_sym = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/Apply#";

        let mut sym_struct = SymbolInformation::default();
        sym_struct.symbol = struct_sym.to_string();
        sym_struct.kind = scip::types::symbol_information::Kind::Struct.into();
        // Two relationships to the same trait — different role mix
        // but both flag is_implementation. The dedupe keys on
        // (from, to), so both should collapse to one fact.
        for _ in 0..2 {
            let mut rel = Relationship::default();
            rel.symbol = trait_sym.to_string();
            rel.is_implementation = true;
            sym_struct.relationships.push(rel);
        }
        doc.symbols.push(sym_struct);

        let mut occ = Occurrence::default();
        occ.symbol = struct_sym.to_string();
        occ.range = vec![0, 0, 0, 4];
        occ.symbol_roles = 1;
        doc.occurrences.push(occ);

        index.documents.push(doc);
        let bytes = index.write_to_bytes().expect("encode");
        let path = dir.join("code.scip");
        std::fs::write(&path, &bytes).unwrap();

        let facts = parse_scip(&path, &dir).unwrap();
        assert_eq!(
            facts.implements.len(),
            1,
            "duplicate is_implementation entries dedupe per (from, to): {:?}",
            facts.implements
        );
    }

    /// Roadmap issue #32 v8 (v0.4.0): when both FROM and TO are
    /// `Trait` kinds, the `is_implementation` relationship classifies
    /// as EXTENDS (trait inheritance) rather than IMPLEMENTS. Asserts
    /// the (Trait, Trait) case lands in `facts.extends` and stays out
    /// of `facts.implements`.
    #[test]
    fn parse_scip_classifies_trait_to_trait_relationship_as_extends() {
        use scip::types::{Document, Occurrence, Relationship, SymbolInformation};

        let dir = std::env::temp_dir().join(format!(
            "doc-linter-scip-extends-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut index = Index::default();
        let mut doc = Document::default();
        doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();
        doc.language = "rust".to_string();

        let parent_trait = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/Validate#";
        let child_trait = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/FastValidate#";

        // Parent trait first.
        let mut sym_parent = SymbolInformation::default();
        sym_parent.symbol = parent_trait.to_string();
        sym_parent.kind = scip::types::symbol_information::Kind::Trait.into();
        doc.symbols.push(sym_parent);

        // Child trait extends parent — relationship FROM child TO
        // parent, is_implementation=true. The classifier sees
        // (Trait, Trait) and routes this to EXTENDS.
        let mut sym_child = SymbolInformation::default();
        sym_child.symbol = child_trait.to_string();
        sym_child.kind = scip::types::symbol_information::Kind::Trait.into();
        let mut rel = Relationship::default();
        rel.symbol = parent_trait.to_string();
        rel.is_implementation = true;
        sym_child.relationships.push(rel);
        doc.symbols.push(sym_child);

        for (s, line) in [(parent_trait, 0), (child_trait, 2)] {
            let mut occ = Occurrence::default();
            occ.symbol = s.to_string();
            occ.range = vec![line, 0, line, 10];
            occ.symbol_roles = 1;
            doc.occurrences.push(occ);
        }

        index.documents.push(doc);
        let bytes = index.write_to_bytes().expect("encode");
        let path = dir.join("code.scip");
        std::fs::write(&path, &bytes).unwrap();

        let facts = parse_scip(&path, &dir).unwrap();
        assert_eq!(
            facts.extends.len(),
            1,
            "trait→trait classifies as EXTENDS: {:?}",
            facts.extends
        );
        let e = &facts.extends[0];
        assert_eq!(e.from_type, child_trait);
        assert_eq!(e.to_type, parent_trait);
        assert!(
            facts.implements.is_empty(),
            "(Trait, Trait) must NOT also land in implements: {:?}",
            facts.implements
        );
    }

    /// Roadmap issue #32 v8: when FROM is `Struct` and TO is `Trait`
    /// (canonical `impl Trait for Struct`), the relationship stays in
    /// IMPLEMENTS — confirms the EXTENDS classifier doesn't over-fire
    /// on the common case.
    #[test]
    fn parse_scip_classifies_struct_to_trait_relationship_as_implements() {
        use scip::types::{Document, Occurrence, Relationship, SymbolInformation};

        let dir = std::env::temp_dir().join(format!(
            "doc-linter-scip-impl-vs-extends-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut index = Index::default();
        let mut doc = Document::default();
        doc.relative_path = "crates/pricing-core/src/lib.rs".to_string();

        let trait_sym = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/Apply#";
        let struct_sym = "rust-analyzer cargo pricing-core 1.0.0 src/lib.rs/Rule#";

        let mut sym_trait = SymbolInformation::default();
        sym_trait.symbol = trait_sym.to_string();
        sym_trait.kind = scip::types::symbol_information::Kind::Trait.into();
        doc.symbols.push(sym_trait);

        let mut sym_struct = SymbolInformation::default();
        sym_struct.symbol = struct_sym.to_string();
        sym_struct.kind = scip::types::symbol_information::Kind::Struct.into();
        let mut rel = Relationship::default();
        rel.symbol = trait_sym.to_string();
        rel.is_implementation = true;
        sym_struct.relationships.push(rel);
        doc.symbols.push(sym_struct);

        for (s, line) in [(trait_sym, 0), (struct_sym, 2)] {
            let mut occ = Occurrence::default();
            occ.symbol = s.to_string();
            occ.range = vec![line, 0, line, 10];
            occ.symbol_roles = 1;
            doc.occurrences.push(occ);
        }

        index.documents.push(doc);
        let bytes = index.write_to_bytes().expect("encode");
        let path = dir.join("code.scip");
        std::fs::write(&path, &bytes).unwrap();

        let facts = parse_scip(&path, &dir).unwrap();
        assert_eq!(
            facts.implements.len(),
            1,
            "(Struct, Trait) stays in IMPLEMENTS: {:?}",
            facts.implements
        );
        assert!(
            facts.extends.is_empty(),
            "EXTENDS classifier must not fire on (Struct, Trait): {:?}",
            facts.extends
        );
    }

    /// Cross-doc classification: the FROM trait lives in one
    /// Document and the TO trait in another. The global symbol →
    /// kind map built in `classify_raw_relationships` must look up
    /// the TO kind across documents to still classify as EXTENDS.
    #[test]
    fn parse_scip_classifies_trait_to_trait_across_documents() {
        use scip::types::{Document, Occurrence, Relationship, SymbolInformation};

        let dir = std::env::temp_dir().join(format!(
            "doc-linter-scip-extends-cross-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut index = Index::default();

        // Doc A defines the parent trait.
        let parent_trait = "rust-analyzer cargo pricing-core 1.0.0 src/parent.rs/Parent#";
        let mut doc_a = Document::default();
        doc_a.relative_path = "crates/pricing-core/src/parent.rs".to_string();
        let mut sym_parent = SymbolInformation::default();
        sym_parent.symbol = parent_trait.to_string();
        sym_parent.kind = scip::types::symbol_information::Kind::Trait.into();
        doc_a.symbols.push(sym_parent);
        let mut occ_parent = Occurrence::default();
        occ_parent.symbol = parent_trait.to_string();
        occ_parent.range = vec![0, 0, 0, 10];
        occ_parent.symbol_roles = 1;
        doc_a.occurrences.push(occ_parent);
        index.documents.push(doc_a);

        // Doc B defines the child trait + the relationship.
        let child_trait = "rust-analyzer cargo pricing-core 1.0.0 src/child.rs/Child#";
        let mut doc_b = Document::default();
        doc_b.relative_path = "crates/pricing-core/src/child.rs".to_string();
        let mut sym_child = SymbolInformation::default();
        sym_child.symbol = child_trait.to_string();
        sym_child.kind = scip::types::symbol_information::Kind::Trait.into();
        let mut rel = Relationship::default();
        rel.symbol = parent_trait.to_string();
        rel.is_implementation = true;
        sym_child.relationships.push(rel);
        doc_b.symbols.push(sym_child);
        let mut occ_child = Occurrence::default();
        occ_child.symbol = child_trait.to_string();
        occ_child.range = vec![0, 0, 0, 10];
        occ_child.symbol_roles = 1;
        doc_b.occurrences.push(occ_child);
        index.documents.push(doc_b);

        let bytes = index.write_to_bytes().expect("encode");
        let path = dir.join("code.scip");
        std::fs::write(&path, &bytes).unwrap();

        let facts = parse_scip(&path, &dir).unwrap();
        assert_eq!(
            facts.extends.len(),
            1,
            "cross-doc (Trait, Trait) still classifies as EXTENDS: {:?}",
            facts.extends
        );
        assert_eq!(facts.extends[0].from_type, child_trait);
        assert_eq!(facts.extends[0].to_type, parent_trait);
    }

    /// The `is_type_descriptor` helper classifies the terminal SCIP
    /// suffix — `#` (Type) is true, `().` (Method/Function) and `.`
    /// (Term) are false, malformed symbols are false.
    #[test]
    fn is_type_descriptor_recognises_type_suffix() {
        assert!(is_type_descriptor(
            "rust-analyzer cargo demo 0.1.0 src/lib.rs/Config#"
        ));
        // Nested namespace then type — still classified as type
        // because the *last* descriptor is the Type suffix.
        assert!(is_type_descriptor(
            "rust-analyzer cargo demo 0.1.0 src/lib.rs/Outer#Inner#"
        ));
        // Method / function descriptors are not type descriptors.
        assert!(!is_type_descriptor(
            "rust-analyzer cargo demo 0.1.0 src/lib.rs/Config#new()."
        ));
        assert!(!is_type_descriptor(
            "rust-analyzer cargo demo 0.1.0 src/lib.rs/run()."
        ));
        // Terms (constants, fields) end with `.` — not types.
        assert!(!is_type_descriptor(
            "rust-analyzer cargo demo 0.1.0 src/lib.rs/MAX."
        ));
        // Malformed symbol — the parser falls through to `false`.
        assert!(!is_type_descriptor("not-a-scip-symbol"));
        assert!(!is_type_descriptor(""));
    }

    #[test]
    fn infer_kind_from_descriptor_recognises_python_shapes() {
        // Free function: trailing `name().`
        assert_eq!(
            infer_kind_from_descriptor("scip-python python p . src/m.py/f()."),
            Some(FunctionKind::Function)
        );
        // Method on a class: `Class#method().`
        assert_eq!(
            infer_kind_from_descriptor("scip-python python p . src/m.py/C#m()."),
            Some(FunctionKind::Method)
        );
        // Class definition: `Class#` (no method after).
        assert_eq!(
            infer_kind_from_descriptor("scip-python python p . src/m.py/C#"),
            Some(FunctionKind::Struct)
        );
        // Term (field / constant) — should NOT be inferred (stays Other).
        assert_eq!(
            infer_kind_from_descriptor("scip-python python p . src/m.py/CONST."),
            None
        );
    }

    #[test]
    fn derive_signature_from_body_extracts_python_def_line() {
        // The spacy and example-app pattern: scip-python leaves
        // signature empty + populates body_excerpt starting with
        // `def function_name(args):`. Iter-191 derives signature
        // from that first line.
        let body = "    def build_options(self):\n        for e in self.extensions:\n            e.extra_compile_args += COMPILE_OPTIONS";
        assert_eq!(
            derive_signature_from_body(body),
            "def build_options(self)",
            "Python def line trimmed + colon stripped",
        );
    }

    #[test]
    fn derive_signature_from_body_handles_blank_leading_lines() {
        // Some SCIP body_excerpts may have blank lines before the
        // def — the helper must skip them and pick the first
        // non-blank.
        let body = "\n\n   \ndef compute(x, y, z):\n    return x + y + z";
        assert_eq!(derive_signature_from_body(body), "def compute(x, y, z)",);
    }

    #[test]
    fn derive_signature_from_body_handles_ts_function_line() {
        // TypeScript/JavaScript shape: `function name(args)` or
        // arrow / method declarations. Iter-191's iter-190
        // counter-validation gap noted TS unmeasured; the helper
        // is shape-agnostic so it works whenever body_excerpt has
        // a function-declaration first line.
        let body = "function dispatch(action: Action): State {\n    switch (action.type) {";
        assert_eq!(
            derive_signature_from_body(body),
            "function dispatch(action: Action): State {",
        );
    }

    #[test]
    fn derive_signature_from_body_returns_empty_on_empty_input() {
        assert_eq!(derive_signature_from_body(""), "");
        assert_eq!(derive_signature_from_body("\n\n   \n"), "");
    }

    #[test]
    fn derive_signature_from_body_strips_only_trailing_colon() {
        // Don't strip a colon that appears mid-line (e.g. Python
        // type-annotated argument).
        let body = "def foo(x: int, y: str) -> bool:";
        assert_eq!(
            derive_signature_from_body(body),
            "def foo(x: int, y: str) -> bool",
            "trailing colon stripped but mid-line colons preserved",
        );
    }
}
