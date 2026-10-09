//! Rust source-comment extractor (Round 3A).
//!
//! Parses a `.rs` file with `tree-sitter` + `tree-sitter-rust` and pulls
//! every doc comment out of the syntax tree, attaching each one to the
//! item it documents (function/struct/enum/trait/impl/mod). The resulting
//! [`CommentExtraction`] is fed through the same `TermIndex` /
//! `resolve_alert` pipeline that the Vale post-processor uses for markdown
//! docs — so prose written in Rust comments is held to the same
//! vocab-closure rules as prose written in `.md` files.
//!
//! ## What counts as a doc comment
//!
//! Tree-sitter's Rust grammar exposes comments as either `line_comment`
//! or `block_comment` nodes. Within those:
//!
//!   - `///` (outer doc) — attaches to the **following** item.
//!   - `//!` (inner doc) — attaches to the **enclosing** item (often the
//!     module / file itself).
//!   - `/** … */` (outer block doc) — same attachment as `///`.
//!   - `/*! … */` (inner block doc) — same attachment as `//!`.
//!   - Plain `//` and `/* */` are NOT doc comments and are skipped.
//!
//! Consecutive `///` line-doc lines are coalesced into a single
//! [`DocComment`] entry: the marker on each line is stripped, the bodies
//! are joined with `\n`, and `line` / `col` point at the *first* line of
//! the run. This matches how rustdoc itself groups them.
//!
//! ## Round 3B handoff
//!
//! Each [`DocComment`] carries `attached_to` so a later round can ingest
//! `Function` (and `Struct`/`Enum`/...) nodes into the store and link them to
//! the entities their comments mention. This module deliberately does NOT
//! touch the store — extraction only.

use anyhow::{Context, Result};
use std::path::Path;
use tree_sitter::{Node, Parser};

/// Common-noun / Rust-keyword stop list shared across the doc-comment
/// vocab linter (Round 3A) and the SCIP symbol-path indexer (Phase 1
/// of roadmap-43). Tokens here are either stdlib types that bleed
/// into prose (`Vec`, `Result`, `HashMap`) or rustdoc section
/// headers (`Returns`, `Errors`, `Panics`) that aren't domain
/// vocabulary. Defined once so all three callers (main.rs,
/// lsp.rs, store.rs) read the same list.
pub const COMMENT_STOPWORDS: &[&str] = &[
    // Rust types / std vocabulary
    "Self",
    "Vec",
    "String",
    "Option",
    "Result",
    "Ok",
    "Err",
    "Some",
    "None",
    "Box",
    "Arc",
    "Rc",
    "Cow",
    "Path",
    "PathBuf",
    "HashMap",
    "HashSet",
    "BTreeMap",
    "BTreeSet",
    "Mutex",
    "RwLock",
    "Cell",
    "RefCell",
    "Iterator",
    "Send",
    "Sync",
    "Sized",
    "Copy",
    "Clone",
    "Default",
    "Debug",
    "Display",
    "Eq",
    "Ord",
    "Hash",
    "From",
    "Into",
    "TryFrom",
    "TryInto",
    "AsRef",
    "AsMut",
    "Deref",
    "DerefMut",
    "Drop",
    "Fn",
    "FnMut",
    "FnOnce",
    "Future",
    "Stream",
    "Pin",
    "Unpin",
    // Python stdlib / typing names that show up in docstrings as type
    // references. Same idea as the Rust block above — these aren't
    // domain vocab, they're language plumbing.
    "True",
    "False",
    "List",
    "Dict",
    "Set",
    "Tuple",
    "Bool",
    "Int",
    "Float",
    "Bytes",
    "Type",
    "Any",
    "Union",
    "Optional",
    "Callable",
    "Iterable",
    "Sequence",
    "Mapping",
    "Generator",
    "Awaitable",
    "Coroutine",
    "Final",
    "Literal",
    "Generic",
    "TypeVar",
    "Protocol",
    "ClassVar",
    // English determiners / common nouns that aren't domain entities
    // but trip the ambiguous-words list
    "Note",
    "Returns",
    "Errors",
    "Panics",
    "Examples",
    "Example",
    "Safety",
    "Arguments",
    "Args",
    "See",
    "Also",
    "TODO",
    "FIXME",
    "NOTE",
    "Warning",
    "Warnings",
    // Sphinx / Google / NumPy docstring section headers — same role
    // as `Returns` / `Errors` above but for Python conventions.
    "Parameters",
    "Yields",
    "Raises",
    "Attributes",
    "Methods",
    "References",
    "Notes",
    // Common English sentence-start words that get capitalised solely
    // because they're the first word of a sentence — not domain nouns.
    // Python docstrings are sentence-prose (PEP-257), so this list
    // matters more there than for terse Rust `///` blocks, but the
    // tokens are universal and would otherwise need to be re-listed
    // by every adopting project.
    "The",
    "This",
    "That",
    "These",
    "Those",
    "There",
    "Here",
    "When",
    "Where",
    "While",
    "Which",
    "What",
    "Who",
    "Why",
    "How",
    "If",
    "Else",
    "Then",
    "Otherwise",
    "Because",
    "Since",
    "Although",
    "However",
    "Therefore",
    "Thus",
    "Hence",
    "Hence",
    "All",
    "Each",
    "Every",
    "Some",
    "Any",
    "Many",
    "Most",
    "Few",
    "Several",
    "Both",
    "Either",
    "Neither",
    "Such",
    "Same",
    "Different",
    "First",
    "Second",
    "Third",
    "Last",
    "Next",
    "Previous",
    "Other",
    "Another",
    "Only",
    "Just",
    "Yes",
    "No",
    "Not",
    "Maybe",
    "Perhaps",
    "Always",
    "Never",
    "Often",
    "Sometimes",
    "Usually",
    "Typically",
    "Generally",
    "Specifically",
    "Especially",
    "Currently",
    "Initially",
    "Finally",
    "Eventually",
    "After",
    "Before",
    "During",
    "Once",
    "Twice",
    "Used",
    "Useful",
    "Use",
    "Using",
    "Run",
    "Runs",
    "Running",
    // Common English connectives / prepositions that fire because the
    // tokenizer matches any `[A-Z][a-z]+`. These would otherwise be
    // re-listed by every project.
    "For",
    "Of",
    "As",
    "By",
    "At",
    "In",
    "On",
    "To",
    "From",
    "With",
    "Without",
    "Into",
    "Onto",
    "Upon",
    "Over",
    "Under",
    "About",
    "Above",
    "Below",
    "After",
    "Around",
    "Across",
    "Through",
    "Throughout",
    "Between",
    "Among",
    "Beyond",
    "Within",
    "Per",
    "Via",
    "Out",
    "Up",
    "Down",
    "Off",
    "It",
    "Its",
    "We",
    "Our",
    "You",
    "Your",
    "They",
    "Their",
    "He",
    "She",
    "His",
    "Her",
    "My",
    "Me",
    "Us",
    "Them",
    "Is",
    "Are",
    "Was",
    "Were",
    "Be",
    "Been",
    "Being",
    "Has",
    "Have",
    "Had",
    "Do",
    "Does",
    "Did",
    "Done",
    "Will",
    "Would",
    "Could",
    "Should",
    "May",
    "Might",
    "Must",
    "Can",
    "And",
    "Or",
    "But",
    "Nor",
    "So",
    "Yet",
    "And",
    "One",
    "Two",
    "Three",
    "Four",
    "Five",
    "Six",
    "Seven",
    "Eight",
    "Nine",
    "Ten",
    "Even",
    "Still",
    "Right",
    "Wrong",
    // Common imperative verbs that start docstring sentences in the
    // Python "Verb-first imperative summary" convention (PEP-257) and
    // also appear in Rust `///` "Returns X" / TS JSDoc "Sends Y" prose.
    // Listed in common inflectional forms because the tokenizer is
    // exact-match (no stemming).
    "Return",
    "Returned",
    "Returning",
    "Add",
    "Adds",
    "Added",
    "Adding",
    "Remove",
    "Removes",
    "Removed",
    "Removing",
    "Detect",
    "Detects",
    "Detected",
    "Detection",
    "Detecting",
    "Classify",
    "Classifies",
    "Classified",
    "Classifying",
    "Classification",
    "Extract",
    "Extracts",
    "Extracted",
    "Extracting",
    "Extraction",
    "Read",
    "Reads",
    "Reading",
    "Write",
    "Writes",
    "Wrote",
    "Written",
    "Writing",
    "Find",
    "Finds",
    "Found",
    "Finding",
    "Compute",
    "Computes",
    "Computed",
    "Computing",
    "Build",
    "Builds",
    "Built",
    "Building",
    "Look",
    "Looks",
    "Looked",
    "Looking",
    "Call",
    "Calls",
    "Called",
    "Calling",
    "Save",
    "Saves",
    "Saved",
    "Saving",
    "Load",
    "Loads",
    "Loaded",
    "Loading",
    "Create",
    "Creates",
    "Created",
    "Creating",
    "Update",
    "Updates",
    "Updated",
    "Updating",
    "Check",
    "Checks",
    "Checked",
    "Checking",
    "Start",
    "Starts",
    "Started",
    "Starting",
    "Stop",
    "Stops",
    "Stopped",
    "Stopping",
    "Parse",
    "Parses",
    "Parsed",
    "Parsing",
    "Apply",
    "Applies",
    "Applied",
    "Applying",
    "Execute",
    "Executes",
    "Executed",
    "Executing",
    "Match",
    "Matches",
    "Matched",
    "Matching",
    "Filter",
    "Filters",
    "Filtered",
    "Filtering",
    "Convert",
    "Converts",
    "Converted",
    "Converting",
    "Process",
    "Processes",
    "Processed",
    "Processing",
    "Handle",
    "Handles",
    "Handled",
    "Handling",
    "Resolve",
    "Resolves",
    "Resolved",
    "Resolving",
    "Open",
    "Opens",
    "Opened",
    "Opening",
    "Close",
    "Closes",
    "Closed",
    "Closing",
    "Send",
    "Sends",
    "Sent",
    "Sending",
    "Receive",
    "Receives",
    "Received",
    "Receiving",
    "Skip",
    "Skips",
    "Skipped",
    "Skipping",
    "Allow",
    "Allows",
    "Allowed",
    "Allowing",
    "Reject",
    "Rejects",
    "Rejected",
    "Rejecting",
    "Accept",
    "Accepts",
    "Accepted",
    "Accepting",
    "Suppress",
    "Suppresses",
    "Suppressed",
    "Suppressing",
    "Emit",
    "Emits",
    "Emitted",
    "Emitting",
    "Yield",
    "Yields",
    "Yielded",
    "Yielding",
    "Init",
    "Initialize",
    "Initializes",
    "Initialized",
    "Reset",
    "Resets",
    "Show",
    "Shows",
    "Shown",
    "Showing",
    "Print",
    "Prints",
    "Printed",
    "Printing",
    "Try",
    "Tries",
    "Tried",
    "Trying",
    "Wait",
    "Waits",
    "Waited",
    "Waiting",
    "Get",
    "Gets",
    "Got",
    "Getting",
    "Set",
    "Sets",
    "Setting",
    "Make",
    "Makes",
    "Made",
    "Making",
    "Take",
    "Takes",
    "Taken",
    "Taking",
    "Give",
    "Gives",
    "Given",
    "Giving",
    "Keep",
    "Keeps",
    "Kept",
    "Keeping",
    "Hold",
    "Holds",
    "Held",
    "Holding",
    "Push",
    "Pushes",
    "Pushed",
    "Pushing",
    "Pull",
    "Pulls",
    "Pulled",
    "Pulling",
    "Pop",
    "Pops",
    "Popped",
    "Popping",
    "Drop",
    "Drops",
    "Dropped",
    "Dropping",
    "Move",
    "Moves",
    "Moved",
    "Moving",
    "Copy",
    "Copies",
    "Copied",
    "Copying",
    "Lift",
    "Lifts",
    "Lifted",
    "Lifting",
    "Construct",
    "Constructs",
    "Constructed",
    // Common technology / protocol acronyms — language-agnostic
    // plumbing names (RFC IDs, file formats, transport protocols).
    "JSON",
    "HTTP",
    "HTTPS",
    "REST",
    "POST",
    "GET",
    "PUT",
    "PATCH",
    "DELETE",
    "API",
    "URL",
    "URI",
    "UUID",
    "SQL",
    "CSV",
    "XML",
    "YAML",
    "TOML",
    "HTML",
    "CSS",
    "IDE",
    "OS",
    "CPU",
    "GPU",
    "RAM",
    "ROM",
    "RPC",
    "DNS",
    "TLS",
    "SSL",
    "SSH",
    "FTP",
    "IP",
    "TCP",
    "UDP",
    "ID",
    "UI",
    "UX",
    "CLI",
    "PID",
    "ASCII",
    "UTF",
    "AST",
    "ORM",
    "REPL",
    "RAG",
    "ML",
    "AI",
    "NN",
    "CI",
    "SHA",
    "ISO",
    "UTC",
    "WHERE",
    "FOR",
    "NOT",
    "AND",
    "OR",
    "IF",
    "THEN",
    "ELSE",
    "TODO",
    "HACK",
    "BUG",
    "FIXME",
    "XXX",
    "HOME",
    "PATH",
    "ENV",
    "PWD",
    "ROOT",
    "VS",
    "OS",
    "Unix",
    "Linux",
    "Android",
    // Common Python exception classes and posix signal names that
    // surface verbatim in prose ("raises a ValueError"). Suffix-based
    // class naming means these tokens hit every project that talks
    // about exception handling.
    "ValueError",
    "TypeError",
    "KeyError",
    "IndexError",
    "OSError",
    "IOError",
    "RuntimeError",
    "AttributeError",
    "ImportError",
    "FileNotFoundError",
    "NotImplementedError",
    "StopIteration",
    "Exception",
    "JSONDecodeError",
    "HTTPError",
    "URLError",
    "SIGINT",
    "SIGTERM",
    "SIGKILL",
    "SIGHUP",
    "SIGQUIT",
    // Additional generic English nouns / verbs that appeared often
    // enough in real-repo prose to be worth a universal entry.
    "An",
    "Begin",
    "Below",
    "Above",
    "Core",
    "Count",
    "Days",
    "Deep",
    "Delete",
    "Derive",
    "Diff",
    "End",
    "Enabled",
    "Ensure",
    "Event",
    "Events",
    "Exact",
    "Explicit",
    "Failure",
    "File",
    "Files",
    "Flags",
    "Footer",
    "Format",
    "Function",
    "General",
    "Generate",
    "Generates",
    "Generated",
    "Generating",
    "Group",
    "Hardware",
    "Header",
    "Hooks",
    "Hosted",
    "Hot",
    "Human",
    "Hypothesis",
    "IDs",
    "Identifies",
    "Identify",
    "Identity",
    "Idempotent",
    "Implement",
    "Import",
    "Index",
    "Infer",
    "Infers",
    "Install",
    "Installation",
    "Installed",
    "Instead",
    "Integration",
    "Intended",
    "Internal",
    "Key",
    "Keys",
    "Knowledge",
    "Label",
    "Language",
    "Length",
    "Library",
    "Lightweight",
    "Local",
    "Logs",
    "Lookup",
    "Manually",
    "Maps",
    "Mark",
    "Maximum",
    "Minimum",
    "Mock",
    "Multiple",
    "Narrative",
    "New",
    "Normalise",
    "Normalises",
    "Number",
    "Numeric",
    "Observer",
    "Operates",
    "Options",
    "Order",
    "Output",
    "Override",
    "Paid",
    "Passed",
    "Payload",
    "Permanent",
    "Pipeline",
    "Port",
    "Preserve",
    "Priority",
    "Produce",
    "Project",
    "Projects",
    "Promote",
    "Provides",
    "Public",
    "Pure",
    "Query",
    "Raw",
    "Reasons",
    "Recency",
    "Record",
    "Register",
    "Registers",
    "Registered",
    "Registration",
    "Replace",
    "Replaces",
    "Replaced",
    "Repository",
    "Request",
    "Response",
    "Restart",
    "Restarts",
    "Restarting",
    "Restrict",
    "Results",
    "Revert",
    "Route",
    "Safe",
    "Schema",
    "Scores",
    "Scrubbed",
    "Session",
    "Settings",
    "Short",
    "Shorten",
    "Sign",
    "Signal",
    "Silently",
    "Simple",
    "Size",
    "Slides",
    "Source",
    "Sources",
    "Specific",
    "Stable",
    "Stack",
    "Stage",
    "States",
    "Staying",
    "Step",
    "Stitches",
    "Stored",
    "Stores",
    "Strategy",
    "Strength",
    "Structure",
    "Stuck",
    "Subset",
    "Sub",
    "Summarise",
    "Summary",
    "Survives",
    "Suspicious",
    "Symptom",
    "Temperature",
    "Timeout",
    "Timestamp",
    "Timestamps",
    "Track",
    "Tuple",
    "Type",
    "Unique",
    "Unknown",
    "Unlike",
    "Validate",
    "Validates",
    "Velocity",
    "Version",
    "Walk",
    "Wall",
    "Watch",
    "Watches",
    "Watching",
    "Well",
    "Worse",
    "Append",
    "Architecture",
    "Aggregate",
    "Aggregates",
    "Assemble",
    "Assembles",
    "Assertions",
    "Bind",
    "Binds",
    "Browser",
    "Cache",
    "Callers",
    "Captures",
    "Category",
    "Cases",
    "Causal",
    "Chain",
    "Change",
    "Clean",
    "Clears",
    "Co",
    "Combines",
    "Command",
    "Compares",
    "Concise",
    "Concrete",
    "Confidence",
    "Consistency",
    "Context",
    "Control",
    "Controlled",
    "Counts",
    "Counting",
    "Critical",
    "Credentials",
    "Current",
    "Database",
    "Decay",
    "Decision",
    "Decorator",
    "Defaults",
    "Description",
    "Design",
    "Designed",
    "Designed",
    "Distinguishes",
    "Diverging",
    "Directory",
    "Domain",
    "Endpoints",
    "Environment",
    "Evidence",
    "Falls",
    "Fetch",
    "Fields",
    "Free",
    "Frequent",
    "Full",
    "Gracefully",
    "Graceful",
    "HEAD",
    "HIGH",
    "Health",
    "HEALTHY",
    "Hyperparameter",
    "Implementation",
    "Initialise",
    "Initialises",
    "Initialised",
    "Hops",
    "Inferred",
    "Pass",
    "Pattern",
    "Persist",
    "Persists",
    "Persisted",
    "Render",
    "Renders",
    "Rendered",
    "Reset",
    "Root",
    "Setup",
    "Shell",
    "Test",
    "Trajectory",
    "Unified",
    "Usage",
    "Uses",
    "WATCH",
    "WHY",
    "WHAT",
    "GiB",
    "MiB",
    "KiB",
    "TiB",
    "GB",
    "MB",
    "KB",
    "TB",
    "Code",
    "Coder",
    "Error",
    "Analyses",
    "Decision",
    "Pre",
    // Common section header words (uppercase variants common in
    // example-app-style docstrings: "FIRST WIN", "PROGRESS BACKED",
    // "GRACEFUL", "MASKING"). They aren't domain vocab.
    "FIRST",
    "PROGRESS",
    "GRACEFUL",
    "MASKING",
    "ESCAPE",
    "ATTEMPT",
    "EVIDENCE",
    "CAUSE",
    "CAUSAL",
    "CHAIN",
    "ACTION",
    "ALL",
    "ONLY",
    "NEVER",
    "ALWAYS",
    "MAYBE",
    "WHEN",
    "WHILE",
    "THAT",
    "OVER",
    "BUT",
    "BACKED",
    "AWARE",
    "FLAGS",
    "STATE",
    "TABLE",
    "WATCH",
    "EXTRACTORS",
    "EXPLICIT",
    "EXTENSIBLE",
    "EXISTS",
    "WITHOUT",
    "IMPLICIT",
    "ROOT",
    "KIND",
    "RESEARCH",
    "TRAJECTORY",
    "THRASHING",
    "LANGUAGE",
    "LOCAL",
    "ONLY",
    "CONFIDENCE",
    "CONTENT",
    "CREATE",
    "CATEGORIES",
    "CATEGORY",
    "SYMPTOM",
    "NEXT",
    // Long-tail additions surfaced by real Python repos — generic
    // English nouns / verbs / adjectives that show up in docstrings
    // but aren't domain vocabulary.
    "English",
    "Absolute",
    "CUDA",
    "Structural",
    "Store",
    "Signature",
    "Column",
    "Outcome",
    "Script",
    "Single",
    "Wraps",
    "Wildcard",
    "Report",
    "POSTs",
    "Entry",
    "SESSION",
    "System",
    "Activity",
    "Ability",
    "Banks",
    "Learning",
    "Architected",
    "Complements",
    "Covers",
    "Data",
    "Describes",
    "Enrich",
    "Paired",
    "Pair",
    "Pairs",
    "Pairing",
    "Feeds",
    "Feed",
    "Fed",
    "Feeding",
    "Rise",
    "Rises",
    "Rising",
    "Rose",
    "Risen",
    "Fall",
    "Falls",
    "Fell",
    "Falling",
    "Fallen",
    "Batch",
    "Batches",
    "Batched",
    "Batching",
    "Variant",
    "Variants",
    "Amortise",
    "Amortised",
    "Amortises",
    "Amortize",
    "Amortized",
];

/// Result of running [`extract_doc_comments`] on one `.rs` file. The
/// extracted blocks become input to the [[entity-doc-graph]] code-comment
/// pipeline (Round 3A) where prose is held to the same vocab-closure rules
/// as `.md` files.
#[derive(Debug, Clone)]
pub struct CommentExtraction {
    pub doc_comments: Vec<DocComment>,
}

/// One doc-comment block. For `///` runs this represents the entire run
/// (multi-line, joined with `\n`); for `//!`, `/** */`, `/*! */` it
/// represents the single comment node. Each block carries the
/// `attached_to` item name so the [[entity-doc-graph]] ingest can build
/// `Function`/`Struct` nodes that link to entities mentioned in the prose.
#[derive(Debug, Clone)]
pub struct DocComment {
    /// Comment body with the leading `///` / `//!` / `/**` / `/*!` and
    /// trailing `*/` markers stripped. Multi-line `///` runs are joined
    /// with `\n`. Leading whitespace inside each line is preserved.
    pub text: String,
    /// 1-based line where the comment (or the first line of a `///` run)
    /// begins.
    pub line: usize,
    /// 1-based column where the comment begins.
    pub col: usize,
    /// Best-effort name of the syntactic item this comment documents.
    /// `None` when no item follows / encloses (e.g. a free-standing
    /// comment block at end-of-file). Inner doc comments resolve to the
    /// nearest enclosing named item, or the special `"<module>"` token
    /// when the comment is at file scope.
    pub attached_to: Option<String>,
    /// Whether the documented symbol is public API — the `MissingAnchor`
    /// rule only fires on public symbols. `false` only where the language
    /// extractor can tell the symbol is private (Python / Dart leading
    /// `_`, C# without `public` / `protected`); Rust and TypeScript
    /// report every documented item as public.
    pub is_public: bool,
}

/// What kind of doc comment this is — needed to decide whether it
/// attaches to the following item (outer) or the enclosing item (inner).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocFlavor {
    /// `///` or `/** */` — documents the next item.
    Outer,
    /// `//!` or `/*! */` — documents the enclosing item / module.
    Inner,
}

/// Extracts doc comments from a Rust source file on disk for ingest into
/// the [[entity-doc-graph]] code-comment pipeline.
pub fn extract_doc_comments(rs_file: &Path) -> Result<CommentExtraction> {
    let content =
        std::fs::read_to_string(rs_file).with_context(|| format!("read {}", rs_file.display()))?;
    extract_doc_comments_from_str(&content)
}

/// Extracts doc comments from in-memory Rust source for the
/// [[entity-doc-graph]] ingest. The parser never reads from disk in this
/// path, so unit tests can call it directly with a string literal and skip
/// touching the filesystem.
pub fn extract_doc_comments_from_str(content: &str) -> Result<CommentExtraction> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::language())
        .context("load tree-sitter-rust grammar")?;
    let tree = parser
        .parse(content, None)
        .context("tree-sitter parse returned None — ungrammatical source")?;
    let bytes = content.as_bytes();

    let mut comments: Vec<DocComment> = Vec::new();
    let mut cursor = tree.walk();
    walk_node(tree.root_node(), bytes, &mut cursor, &mut comments);

    Ok(CommentExtraction {
        doc_comments: comments,
    })
}

/// Recursive descent collecting doc comments scoped to each container.
/// The outer-doc grouping logic walks each container's children list in
/// order: a run of consecutive `///` lines plus any trailing `/** */`
/// block doc gets bundled and the next non-comment item is queried for
/// its name.
fn walk_node<'a>(
    node: Node<'a>,
    bytes: &[u8],
    cursor: &mut tree_sitter::TreeCursor<'a>,
    out: &mut Vec<DocComment>,
) {
    // Collect children once so we can index forward to find the attached
    // item without re-walking the cursor.
    let children: Vec<Node<'a>> = node.children(cursor).collect();

    // Determine the enclosing-item name for `//!` comments inside this
    // node. For the root `source_file` node we use "<module>". For a
    // `mod_item` / `impl_item` / `function_item` / etc. we use the item's
    // own name (best-effort).
    let enclosing_name = enclosing_item_name(node, bytes);

    let mut i = 0;
    while i < children.len() {
        let child = children[i];
        if let Some(flavor) = doc_flavor(child, bytes) {
            // Group consecutive comments of compatible flavor (outer-line
            // runs are the common case). Block doc comments are always
            // single-comment groups; mixing inner+outer isn't grouped.
            let mut group_end = i + 1;
            let mut group_text = stripped_text(child, bytes);
            let group_line = node_line(child);
            let group_flavor = flavor;

            // Only `line_comment` runs get coalesced. Block doc comments
            // stand alone. Detect line-vs-block via the marker prefix.
            if is_line_doc(child, bytes) {
                while group_end < children.len() {
                    let nxt = children[group_end];
                    let Some(nxt_flavor) = doc_flavor(nxt, bytes) else {
                        break;
                    };
                    if nxt_flavor != group_flavor || !is_line_doc(nxt, bytes) {
                        break;
                    }
                    group_text.push('\n');
                    group_text.push_str(&stripped_text(nxt, bytes));
                    group_end += 1;
                }
            }

            let attached = match group_flavor {
                DocFlavor::Outer => {
                    // Look forward past any further comments to the first
                    // item-like node and grab its name.
                    let mut j = group_end;
                    while j < children.len() {
                        let n = children[j];
                        // Skip any non-attached chatter (further comments,
                        // whitespace doesn't appear as a node, attribute
                        // items SHOULD be skipped so doc-comment-then-
                        // `#[derive]`-then-struct still attaches).
                        if doc_flavor(n, bytes).is_some() {
                            j += 1;
                            continue;
                        }
                        if n.kind() == "attribute_item" || n.kind() == "inner_attribute_item" {
                            j += 1;
                            continue;
                        }
                        break;
                    }
                    if j < children.len() {
                        item_name(children[j], bytes)
                    } else {
                        None
                    }
                }
                DocFlavor::Inner => enclosing_name.clone(),
            };

            out.push(DocComment {
                text: group_text,
                line: group_line,
                col: child.start_position().column + 1,
                attached_to: attached,
                is_public: true,
            });

            i = group_end;
            continue;
        }
        // Recurse into non-comment nodes so we find inner doc comments
        // inside `mod foo { //! … }` etc.
        if child.child_count() > 0 {
            let mut sub = child.walk();
            walk_node(child, bytes, &mut sub, out);
        }
        i += 1;
    }
}

/// Returns the name of the syntactic item this node represents, if any.
/// Used both to attach outer doc comments to the following item and to
/// identify the enclosing item for inner doc comments.
fn item_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    match node.kind() {
        "function_item" | "struct_item" | "enum_item" | "trait_item" | "mod_item" | "type_item"
        | "const_item" | "static_item" | "union_item" => {
            let name = node.child_by_field_name("name")?;
            slice(bytes, name).map(str::to_string)
        }
        // `impl A for B` / `impl A` doesn't have a `name:` field — synth
        // a stable identifier from the type.
        "impl_item" => {
            if let Some(type_node) = node.child_by_field_name("type") {
                slice(bytes, type_node).map(|s| format!("impl {s}"))
            } else {
                Some("impl".to_string())
            }
        }
        _ => None,
    }
}

/// Resolves the enclosing-item name used for `//!` / `/*!` attachment.
fn enclosing_item_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    if node.kind() == "source_file" {
        return Some("<module>".to_string());
    }
    item_name(node, bytes)
}

/// Returns Some(flavor) if the node is a doc comment, None otherwise —
/// the [[entity-doc-graph]] tree-sitter walker uses this to decide whether
/// a node attaches to the next item (Outer) or the enclosing one (Inner).
fn doc_flavor(node: Node<'_>, bytes: &[u8]) -> Option<DocFlavor> {
    let kind = node.kind();
    let raw = slice(bytes, node)?;
    match kind {
        "line_comment" => {
            // `///` (outer) but NOT `////` (just a divider).
            if raw.starts_with("///") && !raw.starts_with("////") {
                Some(DocFlavor::Outer)
            } else if raw.starts_with("//!") {
                Some(DocFlavor::Inner)
            } else {
                None
            }
        }
        "block_comment" => {
            // `/**` (outer) but NOT `/***` and NOT `/**/`.
            if raw.starts_with("/**") && !raw.starts_with("/***") && raw != "/**/" {
                Some(DocFlavor::Outer)
            } else if raw.starts_with("/*!") {
                Some(DocFlavor::Inner)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// True when the tree-sitter node is a `///` outer or `//!` inner line doc-comment.
/// Filter used by the [[entity-doc-graph]] code-comments ingest to skip
/// non-rustdoc line comments before mention scanning.
fn is_line_doc(node: Node<'_>, bytes: &[u8]) -> bool {
    if node.kind() != "line_comment" {
        return false;
    }
    let Some(raw) = slice(bytes, node) else {
        return false;
    };
    (raw.starts_with("///") && !raw.starts_with("////")) || raw.starts_with("//!")
}

/// Strips the `///` / `//!` / `/**` / `/*!` / `*/` markers from a comment
/// node's source text and returns the body for the [[entity-doc-graph]]
/// vocab-closure check. Leading whitespace AFTER the marker is preserved
/// (rustdoc tolerates either `///foo` or `/// foo`).
fn stripped_text(node: Node<'_>, bytes: &[u8]) -> String {
    let Some(raw) = slice(bytes, node) else {
        return String::new();
    };
    if let Some(rest) = raw.strip_prefix("///") {
        return rest.trim_start_matches(' ').trim_end().to_string();
    }
    if let Some(rest) = raw.strip_prefix("//!") {
        return rest.trim_start_matches(' ').trim_end().to_string();
    }
    if let Some(rest) = raw.strip_prefix("/**") {
        let body = rest.strip_suffix("*/").unwrap_or(rest);
        return body.trim_start_matches([' ', '\t']).trim_end().to_string();
    }
    if let Some(rest) = raw.strip_prefix("/*!") {
        let body = rest.strip_suffix("*/").unwrap_or(rest);
        return body.trim_start_matches([' ', '\t']).trim_end().to_string();
    }
    raw.to_string()
}

/// Borrows the UTF-8 source slice covered by a tree-sitter node for the
/// [[entity-doc-graph]] code-comment extractor.
fn slice<'a>(bytes: &'a [u8], node: Node<'_>) -> Option<&'a str> {
    let range = node.byte_range();
    bytes.get(range).and_then(|b| std::str::from_utf8(b).ok())
}

/// Converts a tree-sitter zero-based row to a 1-based source line for
/// [[entity-doc-graph]] diagnostics that quote line numbers.
fn node_line(node: Node<'_>) -> usize {
    node.start_position().row + 1
}

/// Source languages whose doc comments feed the [[entity-doc-graph]]
/// code-comment lint. One variant per extractor; JavaScript rides the
/// TypeScript grammar (a superset), JSX the TSX one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    Python,
    CSharp,
    Dart,
    /// Javadoc `/** … */` blocks.
    Java,
    /// Vue single-file component — its `<script>` blocks are TypeScript.
    Vue,
}

impl Lang {
    /// Picks the extractor from the file extension; `None` for files the
    /// comment lint does not read.
    pub fn from_path(path: &Path) -> Option<Self> {
        Some(match path.extension()?.to_str()? {
            "rs" => Self::Rust,
            "ts" | "cts" | "mts" | "js" | "mjs" | "cjs" => Self::TypeScript,
            "tsx" | "jsx" => Self::Tsx,
            "py" => Self::Python,
            "cs" => Self::CSharp,
            "dart" => Self::Dart,
            "java" => Self::Java,
            "vue" => Self::Vue,
            _ => return None,
        })
    }
}

/// Extracts doc comments from any supported source file, dispatching on
/// [`Lang::from_path`]. Unsupported extensions yield no comments.
pub fn extract_any(path: &Path) -> Result<CommentExtraction> {
    let Some(lang) = Lang::from_path(path) else {
        return Ok(CommentExtraction {
            doc_comments: Vec::new(),
        });
    };
    let content =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    extract_any_from_str(lang, &content)
}

/// In-memory twin of [`extract_any`] — the LSP lints unsaved buffers.
pub fn extract_any_from_str(lang: Lang, content: &str) -> Result<CommentExtraction> {
    match lang {
        Lang::Rust => extract_doc_comments_from_str(content),
        Lang::TypeScript => {
            crate::code_comments_ts::extract_doc_comments_ts_from_str(content, false)
        }
        Lang::Tsx => crate::code_comments_ts::extract_doc_comments_ts_from_str(content, true),
        Lang::Python => crate::code_comments_py::extract_doc_comments_py_from_str(content),
        Lang::CSharp | Lang::Dart => Ok(crate::code_comments_slash::extract(content, lang)),
        Lang::Java => Ok(crate::code_comments_java::extract(content)),
        Lang::Vue => extract_vue(content),
    }
}

/// Runs the TypeScript extractor over every `<script>` block of a Vue
/// single-file component, shifting line numbers back to the `.vue` file.
fn extract_vue(content: &str) -> Result<CommentExtraction> {
    let mut doc_comments = Vec::new();
    let mut rest = content;
    let mut offset = 0; // byte offset of `rest` within `content`
    while let Some(open) = rest.find("<script") {
        let Some(tag_end) = rest[open..].find('>') else {
            break;
        };
        let body_start = open + tag_end + 1;
        let Some(close) = rest[body_start..].find("</script>") else {
            break;
        };
        let body = &rest[body_start..body_start + close];
        let lines_before = content[..offset + body_start].matches('\n').count();
        let mut part = crate::code_comments_ts::extract_doc_comments_ts_from_str(body, false)?;
        for c in &mut part.doc_comments {
            c.line += lines_before;
        }
        doc_comments.append(&mut part.doc_comments);
        let consumed = body_start + close + "</script>".len();
        offset += consumed;
        rest = &rest[consumed..];
    }
    Ok(CommentExtraction { doc_comments })
}

/// Roadmap-49 phase 1: one `@endpoint <METHOD> <path>` marker found
/// inside a doc-comment block. Method is uppercased verbatim from the
/// marker (HTTP verbs / `CLI` / `MCP`); path is kept literally as
/// written. `handler_symbol` is the comment's `attached_to` name —
/// the existing tree-sitter pass populates this so resolving the
/// handler is a free byproduct of marker extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointMarker {
    pub method: String,
    pub path: String,
    pub handler_symbol: Option<String>,
    pub source_file: String,
    pub source_line: u32,
}

/// Compiled regex for the [[entity-doc-graph]] `@endpoint <METHOD> <path>`
/// marker grammar per roadmap-49 phase 1a. Anchored to start-of-line (after
/// optional leading whitespace from the doc-comment body); group 1 is the
/// method (uppercase letters only), group 2 is the rest of the line
/// (the path, kept verbatim minus trailing whitespace).
fn endpoint_marker_regex() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Regex is a string literal — compile failure is a build-time bug,
        // not a runtime condition. The expect message documents intent.
        #[allow(
            clippy::expect_used,
            reason = "literal regex; compile is infallible at runtime"
        )]
        regex::Regex::new(r"^\s*@endpoint\s+([A-Z]+)\s+(\S.*?)\s*$")
            .expect("endpoint marker regex compiles")
    })
}

/// Walks every doc-comment in `extraction` for the [[entity-doc-graph]]
/// endpoint pipeline, regex-finds `@endpoint <METHOD> <path>` lines inside
/// each comment body, and emits one [`EndpointMarker`] per match.
/// Multi-line `///` runs may carry more than one marker — both are
/// emitted, sharing the same `handler_symbol` (the comment's
/// `attached_to`). The `source_file` parameter is kept verbatim on every
/// emitted marker; callers pass the repo-relative path of the file the
/// extraction came from.
///
/// Markers' `source_line` is the 1-based source line where the
/// marker text appeared, computed as `comment.line + offset_within_block`.
pub fn extract_endpoint_markers(
    extraction: &CommentExtraction,
    source_file: &str,
) -> Vec<EndpointMarker> {
    let re = endpoint_marker_regex();
    let mut out: Vec<EndpointMarker> = Vec::new();
    for comment in &extraction.doc_comments {
        for (offset, line) in comment.text.lines().enumerate() {
            let Some(caps) = re.captures(line) else {
                continue;
            };
            let Some(method_match) = caps.get(1) else {
                continue;
            };
            let Some(path_match) = caps.get(2) else {
                continue;
            };
            let method = method_match.as_str().to_string();
            let path = path_match.as_str().to_string();
            let line_no = comment.line.saturating_add(offset) as u32;
            out.push(EndpointMarker {
                method,
                path,
                handler_symbol: comment.attached_to.clone(),
                source_file: source_file.to_string(),
                source_line: line_no,
            });
        }
    }
    out
}

/// Tree-sitter doc-comment extraction tests for the [[entity-doc-graph]]
/// code-comment ingest pipeline.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;

    fn extract(content: &str) -> CommentExtraction {
        extract_doc_comments_from_str(content).expect("parse should succeed")
    }

    /// Asserts that an outer `///` doc-comment above a function attaches to
    /// that function in the [[entity-doc-graph]] extraction.
    #[test]
    fn extracts_outer_doc_comment_for_function() {
        let src = "/// foo\nfn bar() {}\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].text, "foo");
        assert_eq!(r.doc_comments[0].attached_to.as_deref(), Some("bar"));
        assert_eq!(r.doc_comments[0].line, 1);
    }

    #[test]
    fn extracts_inner_doc_comment_at_module_level() {
        let src = "//! crate doc\n\nfn x() {}\n";
        let r = extract(src);
        assert!(
            r.doc_comments
                .iter()
                .any(|c| c.text == "crate doc" && c.attached_to.as_deref() == Some("<module>")),
            "{:?}",
            r.doc_comments
        );
    }

    /// Asserts that a `/** ... */` block doc-comment is extracted and
    /// attached just like a `///` line comment in the [[entity-doc-graph]]
    /// pipeline.
    #[test]
    fn block_doc_comment_is_extracted() {
        let src = "/** foo */\nfn bar() {}\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].text, "foo");
        assert_eq!(r.doc_comments[0].attached_to.as_deref(), Some("bar"));
    }

    /// Asserts that a plain `//` line comment is dropped (not a rustdoc) by
    /// the [[entity-doc-graph]] code-comment extractor.
    #[test]
    fn regular_line_comment_is_skipped() {
        let src = "// not a doc\nfn bar() {}\n";
        let r = extract(src);
        assert!(
            r.doc_comments.is_empty(),
            "expected no doc comments, got {:?}",
            r.doc_comments
        );
    }

    /// Asserts that consecutive `///` lines coalesce into a single
    /// doc-comment node in the [[entity-doc-graph]] extraction.
    #[test]
    fn extracts_multiple_attached_doc_lines() {
        let src = "/// foo\n/// bar\nfn x() {}\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].text, "foo\nbar");
        assert_eq!(r.doc_comments[0].attached_to.as_deref(), Some("x"));
    }

    /// Asserts that a `///` followed by `#[derive(...)]` still attaches to
    /// the underlying struct in the [[entity-doc-graph]] extractor.
    #[test]
    fn outer_doc_passes_through_attribute_to_struct() {
        // `///` then `#[derive(Debug)]` then `struct Foo {}` — attachment
        // should still resolve to `Foo`, not the attribute.
        let src = "/// docs\n#[derive(Debug)]\nstruct Foo;\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1);
        assert_eq!(r.doc_comments[0].attached_to.as_deref(), Some("Foo"));
    }

    /// Asserts that a doc-comment above `impl Foo` resolves its attachment
    /// target to `impl Foo` in the [[entity-doc-graph]] extraction.
    #[test]
    fn impl_block_attachment_uses_type_name() {
        let src = "/// the impl\nimpl Foo {}\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1);
        assert_eq!(r.doc_comments[0].attached_to.as_deref(), Some("impl Foo"));
    }

    /// Asserts that `////` (Rust's section divider idiom) is not treated as
    /// a doc-comment by the [[entity-doc-graph]] extractor.
    #[test]
    fn quadruple_slash_is_not_a_doc_comment() {
        // `////` is rust's own "section divider" idiom — explicitly NOT
        // a doc comment.
        let src = "//// divider\nfn x() {}\n";
        let r = extract(src);
        assert!(r.doc_comments.is_empty());
    }

    /// Roadmap-49 phase 1 [[entity-doc-graph]]: a single `/// @endpoint GET /x`
    /// line above a fn yields one EndpointMarker whose method/path/handler
    /// match the marker.
    #[test]
    fn extract_endpoint_markers_finds_get_marker() {
        let src = "/// @endpoint GET /x\nfn handler() {}\n";
        let r = extract(src);
        let markers = extract_endpoint_markers(&r, "crates/x/src/lib.rs");
        assert_eq!(markers.len(), 1, "{markers:?}");
        let m = &markers[0];
        assert_eq!(m.method, "GET");
        assert_eq!(m.path, "/x");
        assert_eq!(m.handler_symbol.as_deref(), Some("handler"));
        assert_eq!(m.source_file, "crates/x/src/lib.rs");
    }

    /// Roadmap-49 phase 1 [[entity-doc-graph]]: two `@endpoint` lines on
    /// the same handler emit two markers, both attached to the same
    /// symbol — the "GET /x and POST /x" idiom.
    #[test]
    fn extract_endpoint_markers_handles_multiple_per_handler() {
        let src = "/// @endpoint GET /x\n\
                   /// @endpoint POST /x\n\
                   fn handler() {}\n";
        let r = extract(src);
        let markers = extract_endpoint_markers(&r, "crates/x/src/lib.rs");
        assert_eq!(markers.len(), 2, "{markers:?}");
        let methods: Vec<&str> = markers.iter().map(|m| m.method.as_str()).collect();
        assert!(methods.contains(&"GET"));
        assert!(methods.contains(&"POST"));
        for m in &markers {
            assert_eq!(m.handler_symbol.as_deref(), Some("handler"));
            assert_eq!(m.path, "/x");
        }
    }

    /// Roadmap-49 phase 1 [[entity-doc-graph]]: an `@endpoint` written in
    /// a plain `//` line comment (NOT a doc-comment) yields zero markers
    /// — the extractor only sees `///` / `//!` content.
    #[test]
    fn extract_endpoint_markers_skips_text_outside_doc_comments() {
        let src = "// @endpoint GET /x\nfn handler() {}\n";
        let r = extract(src);
        let markers = extract_endpoint_markers(&r, "crates/x/src/lib.rs");
        assert!(markers.is_empty(), "{markers:?}");
    }

    /// Roadmap-49 phase 1 [[entity-doc-graph]]: the path is kept verbatim
    /// — params, query strings, and special characters survive untouched.
    #[test]
    fn extract_endpoint_markers_preserves_path_verbatim() {
        let src = "/// @endpoint POST /v1/outlets/:id/prices?foo=bar\n\
                   fn handler() {}\n";
        let r = extract(src);
        let markers = extract_endpoint_markers(&r, "crates/x/src/lib.rs");
        assert_eq!(markers.len(), 1, "{markers:?}");
        assert_eq!(markers[0].path, "/v1/outlets/:id/prices?foo=bar");
        assert_eq!(markers[0].method, "POST");
    }
}
