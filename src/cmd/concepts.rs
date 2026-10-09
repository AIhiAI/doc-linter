//! `doc-linter ontology propose` / `ontology accept`
//! (docs/design/local-concept-proposer.md, plan items C5 + C3).
//!
//! `propose` groups the graph's Functions and Types into named candidate
//! concepts with no network and no API key: CALLS edges plus rare-token
//! affinity edges (plus optional stored-embedding edges) feed the existing
//! community detection in `cmd::cluster`, and TF-IDF over identifier and
//! doc-comment tokens names each community. An optional local
//! `[concepts] namer_command` may rename a candidate; any failure falls
//! back to the TF-IDF name. `propose` writes only the proposals file;
//! `accept` reads that file plus the graph and writes an ontology entity
//! AND a narrative doc built from the real code. There is deliberately no
//! `accept --all`: every entity must come with the narrative that
//! justifies it, and a cluster too thin to cite writes nothing.
//!
//! Both engines work: everything is read through `store::typed`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use doc_linter::config::LintConfig;
use doc_linter::ontology::Ontology;
use doc_linter::store::symbol_tokens;
use doc_linter::store::typed::{self, ConceptSymbol};

/// Where `propose` writes and `accept` reads by default.
pub(crate) const DEFAULT_PROPOSALS: &str = ".doc-lint/proposals.json";

/// Cosine similarity at or above which two stored vectors share an edge.
const EMBEDDING_EDGE_THRESHOLD: f32 = 0.80;
/// Beyond this many vectors the O(n^2) edge pass is skipped.
const EMBEDDING_MAX_NODES: usize = 4000;
/// A token held by at most this many symbols joins them all pairwise;
/// above it, only neighbours in sorted order.
const CLIQUE_MAX: usize = 10;

// --- words, stop lists --------------------------------------------------

/// Generic words that never name a concept (verbs, filler, Rust and
/// layout vocabulary). Single domain nouns such as `user` are NOT here:
/// a one-word name is allowed, this list only rejects the generic ones.
const GENERIC: &[&str] = &[
    "get", "set", "new", "impl", "handle", "handler", "helper", "helpers", "util", "utils", "test",
    "tests", "mod", "main", "lib", "src", "common", "base", "default", "data", "info", "item",
    "items", "list", "type", "types", "error", "errors", "result", "results", "value", "values",
    "config", "manager", "service", "create", "update", "delete", "remove", "add", "make", "build",
    "init", "run", "exec", "execute", "process", "read", "write", "load", "save", "open", "close",
    "start", "stop", "find", "check", "parse", "format", "convert", "display", "debug", "clone",
    "drop", "from", "into", "with", "the", "and", "for", "that", "this", "not", "are", "was",
    "use", "used", "uses", "using", "can", "will", "has", "have", "its", "all", "any", "one",
    "two", "when", "then", "else", "also", "each", "only", "more", "some", "such", "than", "them",
    "they", "their", "there", "which", "what", "who", "how", "why", "where", "while", "must",
    "should", "would", "could", "may", "might", "return", "returns", "returned", "self", "mut",
    "pub", "crate", "fixture", "internal", "private", "public", "object", "string", "bool", "true",
    "false", "none", "some", "text", "name", "names", "key", "keys", "path", "file", "files",
    "line", "lines", "member", "cs",
];

fn stop_list(config: &LintConfig) -> HashSet<String> {
    let mut s: HashSet<String> = GENERIC.iter().map(|w| (*w).to_string()).collect();
    // ponytail: `code_comments::COMMENT_STOPWORDS` (Rust type vocabulary) is
    // not merged: it lists real domain nouns such as Session and Report.
    for w in &config.vale.vale_ambiguous_words {
        s.insert(w.to_ascii_lowercase());
    }
    s
}

/// Lowercased words of an identifier, path or sentence: split on
/// non-alphanumerics, camelCase and acronym boundaries.
fn split_words(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if c.is_uppercase() && !cur.is_empty() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || prev.is_ascii_digit() || (prev.is_uppercase() && next_lower) {
                out.push(std::mem::take(&mut cur));
            }
        }
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Leaf identifier of a SCIP symbol: `pkg src/a.rs/Type#method().` ->
/// `method`; `.../create_invoice().` -> `create_invoice`.
fn leaf_name(symbol: &str) -> String {
    let last = symbol.rsplit('/').next().unwrap_or(symbol);
    let cleaned = last.trim_end_matches(['.', '(', ')']);
    let method = cleaned.rsplit('#').next().unwrap_or(cleaned);
    let pick = if method.is_empty() {
        cleaned.trim_end_matches('#')
    } else {
        method
    };
    pick.trim_matches(['`', '#', '.', ' ']).to_string()
}

/// `Type::method` / `function` for display in the narrative.
fn short_name(symbol: &str) -> String {
    let last = symbol.rsplit('/').next().unwrap_or(symbol);
    let cleaned = last.trim_end_matches(['.', '(', ')']);
    cleaned
        .replace('#', "::")
        .trim_matches(['`', ':'])
        .to_string()
}

// --- per-symbol token info ----------------------------------------------

struct Info {
    sym: ConceptSymbol,
    /// Tokens of the symbol, its directories and its file stem.
    ident: BTreeSet<String>,
    /// Tokens of the doc comment.
    doc: BTreeSet<String>,
    leaf_words: Vec<String>,
}

impl Info {
    fn new(sym: ConceptSymbol, stops: &HashSet<String>) -> Self {
        let keep = |w: &String| w.len() >= 3 && !stops.contains(w);
        let mut ident: BTreeSet<String> = symbol_tokens(&sym.symbol)
            .into_iter()
            .filter(&keep)
            .collect();
        let path = sym
            .file
            .rsplit_once('.')
            .map_or(sym.file.as_str(), |(p, _)| p);
        ident.extend(split_words(path).into_iter().filter(&keep));
        let doc: BTreeSet<String> = split_words(&sym.doc_comment)
            .into_iter()
            .filter(|w| w.len() >= 4 && !stops.contains(w))
            .collect();
        let leaf_words = split_words(&leaf_name(&sym.symbol));
        Info {
            sym,
            ident,
            doc,
            leaf_words,
        }
    }

    fn has(&self, w: &str) -> bool {
        self.ident.contains(w) || self.doc.contains(w)
    }
}

// --- edges ---------------------------------------------------------------

/// Pairs of indices that share a rare token. A token held by one symbol
/// joins nothing; one held by more than `cap` is too common to say
/// anything and joins nothing either.
fn affinity_edges(tokens: &[BTreeSet<String>], cap: usize) -> BTreeSet<(usize, usize)> {
    let mut holders: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, set) in tokens.iter().enumerate() {
        for t in set {
            holders.entry(t.as_str()).or_default().push(i);
        }
    }
    let mut out = BTreeSet::new();
    for hs in holders.values() {
        if hs.len() < 2 || hs.len() > cap {
            continue;
        }
        if hs.len() <= CLIQUE_MAX {
            for (k, &a) in hs.iter().enumerate() {
                for &b in &hs[k + 1..] {
                    out.insert((a, b));
                }
            }
        } else {
            for (k, &a) in hs.iter().enumerate() {
                for &b in hs.iter().skip(k + 1).take(2) {
                    out.insert((a, b));
                }
            }
        }
    }
    out
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

// --- naming --------------------------------------------------------------

struct Ranked {
    /// Valid-looking names, best first (bigrams, then single tokens).
    names: Vec<String>,
    /// Single tokens by score, best first.
    tokens: Vec<String>,
}

/// TF-IDF over member tokens. Identifier tokens weigh 1 (2 on a Type),
/// doc-comment tokens 1.5; IDF is against every symbol in the graph. A
/// bigram of adjacent words in the symbol names that at least half the
/// members share outranks single tokens.
fn rank_names(members: &[&Info], df: &HashMap<String, usize>, total: usize) -> Ranked {
    let mut score: BTreeMap<&str, f64> = BTreeMap::new();
    for m in members {
        let ident_w = if m.sym.is_type { 2.0 } else { 1.0 };
        for t in &m.ident {
            *score.entry(t).or_insert(0.0) += ident_w;
        }
        for t in &m.doc {
            *score.entry(t).or_insert(0.0) += 1.5;
        }
    }
    let idf = |t: &str| (total as f64 / df.get(t).copied().unwrap_or(1).max(1) as f64).ln_1p();
    let mut tokens: Vec<(&str, f64)> = score.iter().map(|(t, s)| (*t, s * idf(t))).collect();
    tokens.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));

    let mut bigrams: BTreeMap<String, (usize, f64)> = BTreeMap::new();
    for m in members {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for w in m.leaf_words.windows(2) {
            if w.iter().all(|x| score.contains_key(x.as_str())) {
                seen.insert(format!("{}-{}", w[0], w[1]));
            }
        }
        for b in seen {
            let (a, c) = b.split_once('-').unwrap_or((&b, ""));
            let weight = idf(a) + idf(c);
            let e = bigrams.entry(b).or_insert((0, weight));
            e.0 += 1;
        }
    }
    let half = members.len().div_ceil(2).max(2);
    let mut bg: Vec<(String, (usize, f64))> = bigrams
        .into_iter()
        .filter(|(_, (n, _))| *n >= half)
        .collect();
    bg.sort_by(|a, b| {
        b.1 .0
            .cmp(&a.1 .0)
            .then_with(|| b.1 .1.total_cmp(&a.1 .1))
            .then_with(|| a.0.cmp(&b.0))
    });

    let single: Vec<String> = tokens
        .iter()
        .filter(|(t, _)| t.len() >= 4)
        .map(|(t, _)| (*t).to_string())
        .collect();
    let mut names: Vec<String> = bg.into_iter().map(|(b, _)| b).collect();
    names.extend(single.iter().cloned());
    Ranked {
        names,
        tokens: tokens.into_iter().map(|(t, _)| t.to_string()).collect(),
    }
}

fn valid_name(n: &str, stops: &HashSet<String>, existing: &HashSet<String>) -> bool {
    (4..=40).contains(&n.len())
        && n.starts_with(|c: char| c.is_ascii_lowercase())
        && !n.ends_with('-')
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !existing.contains(n)
        && n.split('-').all(|p| !p.is_empty() && !stops.contains(p))
}

// --- proposals file -------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct FileCount {
    pub file: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Proposal {
    pub name: String,
    pub alt_names: Vec<String>,
    pub confidence: f64,
    pub member_count: usize,
    /// `tfidf` or `namer_command`.
    pub namer: String,
    pub description: String,
    pub synonyms: Vec<String>,
    pub top_files: Vec<FileCount>,
    pub sample_doc_comment: String,
    pub source_files: Vec<String>,
    pub members: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProposalsFile {
    version: u32,
    proposals: Vec<Proposal>,
}

pub(crate) struct ProposeArgs {
    pub top_n: usize,
    pub min_members: usize,
    pub algorithm: String,
    pub embeddings: bool,
    pub json: bool,
    pub out: PathBuf,
}

fn abs(root: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    }
}

fn existing_names(ontology: &Ontology) -> HashSet<String> {
    let mut s = HashSet::new();
    for e in ontology.entities.values() {
        s.insert(e.id.to_ascii_lowercase());
        for syn in &e.synonyms {
            s.insert(syn.to_ascii_lowercase());
        }
    }
    s
}

fn first_line(s: &str, max: usize) -> String {
    let l = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    l.chars().take(max).collect()
}

/// In-degree of each symbol over the CALLS edges among `known`.
fn in_degrees(calls: &[(String, String)], known: &HashSet<&str>) -> HashMap<String, usize> {
    let mut deg: HashMap<String, usize> = HashMap::new();
    for (a, b) in calls {
        if a != b && known.contains(a.as_str()) && known.contains(b.as_str()) {
            *deg.entry(b.clone()).or_insert(0) += 1;
        }
    }
    deg
}

// --- namer hook -------------------------------------------------------------

fn namer_timeout() -> Duration {
    let ms = std::env::var("DOC_LINTER_NAMER_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(10_000);
    Duration::from_millis(ms)
}

/// Run the user's local namer: one JSON object on stdin, `{"name",
/// "description"}` expected on stdout. `None` on any failure or timeout.
fn run_namer(command: &str, payload: &serde_json::Value) -> Option<(String, String)> {
    let mut parts = command.split_whitespace();
    let program = parts.next()?;
    let mut child = Command::new(program)
        .args(parts)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let line = format!("{payload}\n");
    // A namer that never reads stdin must not block us: write on a thread.
    std::thread::spawn(move || {
        let _ = stdin.write_all(line.as_bytes());
    });
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut buf);
        let _ = tx.send(buf);
    });
    let Ok(out) = rx.recv_timeout(namer_timeout()) else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let _ = child.wait();
    let v: serde_json::Value = serde_json::from_str(out.trim()).ok()?;
    let name = v.get("name")?.as_str()?.trim().to_ascii_lowercase();
    let description = v
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    Some((name, description))
}

// --- propose ----------------------------------------------------------------

/// Build the proposals for already-loaded graph facts. Pure and
/// deterministic: the same inputs give the same list in the same order.
#[allow(
    clippy::too_many_arguments,
    reason = "one cohesive pass; a params struct would only rename the same inputs"
)]
fn build_proposals(
    symbols: Vec<ConceptSymbol>,
    calls: &[(String, String)],
    vectors: &[(String, Vec<f32>)],
    stops: &HashSet<String>,
    existing: &HashSet<String>,
    namer: Option<&str>,
    args: &ProposeArgs,
) -> Result<Vec<Proposal>> {
    let infos: Vec<Info> = symbols.into_iter().map(|s| Info::new(s, stops)).collect();
    let total = infos.len();
    let index: HashMap<&str, usize> = infos
        .iter()
        .enumerate()
        .map(|(i, m)| (m.sym.symbol.as_str(), i))
        .collect();

    let mut df: HashMap<String, usize> = HashMap::new();
    for m in &infos {
        for t in m.ident.union(&m.doc) {
            *df.entry(t.clone()).or_insert(0) += 1;
        }
    }

    let mut edges: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (a, b) in calls {
        if let (Some(&i), Some(&j)) = (index.get(a.as_str()), index.get(b.as_str())) {
            if i != j {
                edges.insert((i.min(j), i.max(j)));
            }
        }
    }
    let token_sets: Vec<BTreeSet<String>> = infos
        .iter()
        .map(|m| m.ident.union(&m.doc).cloned().collect())
        .collect();
    edges.extend(affinity_edges(&token_sets, (total / 25).max(8)));
    for (i, (sa, va)) in vectors.iter().enumerate() {
        for (sb, vb) in &vectors[i + 1..] {
            if cosine(va, vb) >= EMBEDDING_EDGE_THRESHOLD {
                if let (Some(&x), Some(&y)) = (index.get(sa.as_str()), index.get(sb.as_str())) {
                    edges.insert((x.min(y), x.max(y)));
                }
            }
        }
    }

    let ids: Vec<String> = infos.iter().map(|m| m.sym.symbol.clone()).collect();
    let edge_ids: Vec<(String, String)> = edges
        .iter()
        .map(|&(i, j)| (ids[i].clone(), ids[j].clone()))
        .collect();
    let communities =
        super::cluster::communities_of(&ids, &edge_ids, &args.algorithm, args.min_members)?;

    let known: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let degree = in_degrees(calls, &known);

    let mut taken: HashSet<String> = HashSet::new();
    let mut out: Vec<Proposal> = Vec::new();
    for members_ids in communities {
        let members: Vec<&Info> = members_ids
            .iter()
            .filter_map(|s| index.get(s.as_str()).map(|&i| &infos[i]))
            .collect();
        let ranked = rank_names(&members, &df, total);
        let usable = |n: &String| valid_name(n, stops, existing) && !taken.contains(n);
        let mut choices = ranked.names.iter().filter(|n| usable(n));
        let Some(mut name) = choices.next().cloned() else {
            eprintln!(
                "doc-linter: dropped a {}-symbol candidate around `{}`: no usable name \
                 (every top token is generic or already an entity)",
                members.len(),
                members_ids.first().map_or("", String::as_str)
            );
            continue;
        };
        let mut alt_names: Vec<String> = choices.take(3).cloned().collect();
        let mut description = String::new();
        let mut namer_used = "tfidf";

        let mut by_degree: Vec<&Info> = members.clone();
        by_degree.sort_by(|a, b| {
            let da = degree.get(&a.sym.symbol).copied().unwrap_or(0);
            let db = degree.get(&b.sym.symbol).copied().unwrap_or(0);
            db.cmp(&da).then_with(|| a.sym.symbol.cmp(&b.sym.symbol))
        });

        if let Some(cmd) = namer {
            let payload = serde_json::json!({
                "tokens": ranked.tokens.iter().take(10).collect::<Vec<_>>(),
                "doc_comments": by_degree.iter()
                    .filter(|m| !m.sym.doc_comment.trim().is_empty())
                    .take(3)
                    .map(|m| m.sym.doc_comment.chars().take(300).collect::<String>())
                    .collect::<Vec<_>>(),
                "symbols": by_degree.iter().take(8)
                    .map(|m| short_name(&m.sym.symbol)).collect::<Vec<_>>(),
            });
            if let Some((n, d)) = run_namer(cmd, &payload) {
                if usable(&n) {
                    alt_names.insert(0, name.clone());
                    alt_names.truncate(3);
                    name = n;
                    description = d;
                    namer_used = "namer_command";
                }
            }
        }
        taken.insert(name.clone());

        let parts: Vec<&str> = name.split('-').collect();
        let with_name = members
            .iter()
            .filter(|m| parts.iter().all(|p| m.has(p)))
            .count();
        let confidence = ((with_name as f64 / members.len() as f64) * 100.0).round() / 100.0;

        let mut files: BTreeMap<&str, usize> = BTreeMap::new();
        for m in &members {
            if !m.sym.file.is_empty() {
                *files.entry(m.sym.file.as_str()).or_insert(0) += 1;
            }
        }
        let mut by_count: Vec<(&str, usize)> = files.iter().map(|(f, c)| (*f, *c)).collect();
        by_count.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let top_files: Vec<FileCount> = by_count
            .iter()
            .take(3)
            .map(|(f, c)| FileCount {
                file: (*f).to_string(),
                count: *c,
            })
            .collect();

        let synonyms: Vec<String> = ranked
            .tokens
            .iter()
            .filter(|t| t.len() >= 4 && !parts.contains(&t.as_str()) && !existing.contains(*t))
            .take(3)
            .cloned()
            .collect();
        let sample = by_degree
            .iter()
            .find(|m| !m.sym.doc_comment.trim().is_empty())
            .map(|m| first_line(&m.sym.doc_comment, 120))
            .unwrap_or_default();
        if description.is_empty() {
            let around: Vec<&str> = ranked.tokens.iter().take(4).map(String::as_str).collect();
            description = format!(
                "Code cluster of {} symbols across {} file(s), centred on {}.",
                members.len(),
                files.len(),
                around.join(", ")
            );
        }
        out.push(Proposal {
            name,
            alt_names,
            confidence,
            member_count: members.len(),
            namer: namer_used.to_string(),
            description,
            synonyms,
            top_files,
            sample_doc_comment: sample,
            source_files: files.keys().take(50).map(|f| (*f).to_string()).collect(),
            members: members_ids.clone(),
        });
    }
    out.sort_by(|a, b| {
        b.member_count
            .cmp(&a.member_count)
            .then_with(|| a.name.cmp(&b.name))
    });
    out.truncate(args.top_n);
    Ok(out)
}

fn open_graph(root: &Path) -> Result<doc_linter::graph_read::ReadGraph> {
    doc_linter::graph_read::ReadGraph::open(root)
        .context("no graph to read: run `doc-linter check` first")
}

pub(crate) fn propose(
    root: &Path,
    config: &LintConfig,
    ontology: &Ontology,
    args: &ProposeArgs,
) -> Result<ExitCode> {
    let db = open_graph(root)?;
    let proposals = collect_proposals(&*db, config, ontology, args)?;

    let file = ProposalsFile {
        version: 1,
        proposals,
    };
    let json = serde_json::to_string_pretty(&file)?;
    let out_path = abs(root, &args.out);
    if let Some(dir) = out_path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    std::fs::write(&out_path, format!("{json}\n"))
        .with_context(|| format!("write {}", out_path.display()))?;

    if args.json {
        println!("{json}");
        return Ok(ExitCode::SUCCESS);
    }
    if file.proposals.is_empty() {
        println!(
            "No candidate concepts found (min-members {}).",
            args.min_members
        );
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "{:<22} {:>7} {:>5}  {:<44} SAMPLE DOC COMMENT",
        "NAME", "MEMBERS", "CONF", "TOP FILES"
    );
    for p in &file.proposals {
        let files: Vec<String> = p
            .top_files
            .iter()
            .take(2)
            .map(|f| format!("{} ({})", f.file, f.count))
            .collect();
        println!(
            "{:<22} {:>7} {:>5.2}  {:<44} {}",
            p.name,
            p.member_count,
            p.confidence,
            files.join(", "),
            p.sample_doc_comment
        );
    }
    println!(
        "\nWrote {} proposal(s) to {}. Nothing under docs/ was changed.\n\
         Accept one with: doc-linter ontology accept <name>   (rename: --rename name=other)",
        file.proposals.len(),
        out_path.display()
    );
    Ok(ExitCode::SUCCESS)
}

/// Read the graph and build the proposals; shared by `propose` and `report`.
pub(crate) fn collect_proposals(
    conn: &(impl doc_linter::store::StoreRead + ?Sized),
    config: &LintConfig,
    ontology: &Ontology,
    args: &ProposeArgs,
) -> Result<Vec<Proposal>> {
    let symbols = typed::concept_symbols(conn);
    if symbols.is_empty() {
        return Err(anyhow!(
            "the graph has no Function or Type rows: run `doc-linter scip-index` \
             then `doc-linter check` first"
        ));
    }
    let calls = typed::concept_calls(conn);
    let vectors = if args.embeddings {
        let v = typed::concept_embeddings(conn);
        if v.is_empty() {
            eprintln!(
                "doc-linter: --embeddings: no readable stored vectors in this graph \
                 (vectors live in the vector index, not in a readable column); \
                 proposing without embedding edges"
            );
            v
        } else if v.len() > EMBEDDING_MAX_NODES {
            eprintln!(
                "doc-linter: --embeddings: {} vectors exceed the {EMBEDDING_MAX_NODES} limit \
                 of the pairwise pass; proposing without embedding edges",
                v.len()
            );
            Vec::new()
        } else {
            v
        }
    } else {
        Vec::new()
    };
    let stops = stop_list(config);
    let existing = existing_names(ontology);
    let proposals = build_proposals(
        symbols,
        &calls,
        &vectors,
        &stops,
        &existing,
        config.concepts.namer_command.as_deref(),
        args,
    )?;
    Ok(proposals)
}

// --- accept -----------------------------------------------------------------

pub(crate) struct AcceptArgs {
    pub names: Vec<String>,
    pub renames: Vec<String>,
    pub dry_run: bool,
    pub from: PathBuf,
}

fn valid_slug(n: &str) -> bool {
    (3..=40).contains(&n.len())
        && n.starts_with(|c: char| c.is_ascii_lowercase())
        && !n.ends_with('-')
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn yaml_str(s: &str) -> String {
    // A JSON string is a valid YAML double-quoted scalar.
    serde_json::to_string(&s.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|_| "\"\"".to_string())
}

/// First sentence of a doc comment, flattened to one line.
fn first_sentence(s: &str) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.find(". ") {
        Some(i) => flat[..=i].to_string(),
        None => flat,
    }
}

/// Keep verbatim comment text from forming wikilinks or fence breaks.
fn defang(s: &str) -> String {
    s.replace("[[", "[ [").replace("]]", "] ]")
}

fn fence_for(s: &str) -> String {
    let mut n = 3;
    while s.contains(&"~".repeat(n)) {
        n += 1;
    }
    "~".repeat(n)
}

const NO_DOC_ROLE: &str =
    "this repo has no ontology (no `doc` role registered): run `doc-linter init` first";

/// First `preferred` value that is registered (and allowed), else the
/// alphabetically first allowed registered value.
fn pick_value<'a>(
    registered: impl Iterator<Item = &'a String>,
    preferred: &[&str],
    allowed: Option<&[String]>,
) -> Option<&'a str> {
    let mut ok: Vec<&str> = registered
        .map(String::as_str)
        .filter(|v| allowed.is_none_or(|a| a.iter().any(|x| x == v)))
        .collect();
    ok.sort_unstable();
    preferred
        .iter()
        .find_map(|p| ok.iter().copied().find(|v| v == p))
        .or_else(|| ok.first().copied())
}

struct Narrative {
    entity_md: String,
    explanation_md: String,
}

/// Why a concept cannot be written, or the two documents. Pure: takes the
/// graph facts as arguments so the thin-cluster rule is unit-testable.
fn render_concept(
    root: &Path,
    p: &Proposal,
    name: &str,
    facts: &HashMap<&str, &ConceptSymbol>,
    calls: &[(String, String)],
    today: &str,
    ontology: &Ontology,
) -> std::result::Result<Narrative, String> {
    let members: Vec<&ConceptSymbol> = p
        .members
        .iter()
        .filter_map(|s| facts.get(s.as_str()).copied())
        .collect();
    let citeable =
        |m: &ConceptSymbol| !m.doc_comment.trim().is_empty() || !m.signature.trim().is_empty();
    if members.iter().filter(|m| citeable(m)).count() < 2 {
        return Err(format!(
            "too thin to document: fewer than 2 of its {} member(s) still in the graph have a \
             signature or doc comment to cite (re-run `doc-linter check`, or document the code first)",
            members.len()
        ));
    }
    let known: HashSet<&str> = members.iter().map(|m| m.symbol.as_str()).collect();
    let degree = in_degrees(calls, &known);
    let deg = |m: &ConceptSymbol| degree.get(&m.symbol).copied().unwrap_or(0);
    let mut ranked: Vec<&ConceptSymbol> = members.clone();
    ranked.sort_by(|a, b| deg(b).cmp(&deg(a)).then_with(|| a.symbol.cmp(&b.symbol)));

    let bullet = |m: &ConceptSymbol| {
        let mut line = format!("- `{}`", short_name(&m.symbol));
        if !m.signature.trim().is_empty() {
            line.push_str(&format!(": `{}`", first_line(&m.signature, 160)));
        }
        let doc = first_line(&m.doc_comment, 160);
        if !doc.is_empty() {
            line.push_str(&format!(" -- {}", defang(&doc)));
        }
        if !m.file.is_empty() {
            line.push_str(&format!(" ({}:{})", m.file, m.line));
        }
        line
    };

    let best_doc = ranked
        .iter()
        .find(|m| !m.doc_comment.trim().is_empty())
        .map(|m| first_sentence(&m.doc_comment));
    let summary = best_doc.unwrap_or_else(|| p.description.clone());

    // Only values the repo's ontology registers, else the doc fails lint.
    let role = ontology.roles.get("doc").ok_or(NO_DOC_ROLE)?;
    let kind = pick_value(ontology.kinds.keys(), &["explanation", "reference"], None)
        .ok_or("the ontology registers no kind to give the narrative")?;
    let lifecycle = pick_value(
        ontology.lifecycles.keys(),
        &["draft", "implementing"],
        role.allowed_lifecycle.as_deref(),
    )
    .ok_or("the ontology registers no lifecycle the `doc` role allows")?;

    let mut md = String::new();
    md.push_str(&format!(
        "---\nid: concept-{name}\nrole: doc\nkind: {kind}\nlifecycle: {lifecycle}\n\
         title: {title}\nsummary: {summary}\nstatus: draft\nupdated: {today}\ncovers: [{name}]\n---\n\n",
        title = yaml_str(&format!("{name}: how it works in the code")),
        summary = yaml_str(&summary),
    ));
    md.push_str(&format!(
        "# {name}\n\nConcept [[entity-{name}]], proposed from the code graph by `doc-linter ontology \
         propose` ({} symbols across {} file(s)).\n\n{}\n\n",
        members.len(),
        p.source_files.len(),
        defang(&summary)
    ));

    md.push_str("## What it contains\n\n");
    let fns: Vec<&&ConceptSymbol> = ranked.iter().filter(|m| !m.is_type).take(5).collect();
    if !fns.is_empty() {
        md.push_str("Most-called functions first; the number in square brackets is how many other members call it:\n\n");
        for m in fns {
            md.push_str(&bullet(m));
            md.push_str(&format!(" [{}]\n", deg(m)));
        }
        md.push('\n');
    }
    let types: Vec<&&ConceptSymbol> = ranked.iter().filter(|m| m.is_type).take(5).collect();
    if !types.is_empty() {
        md.push_str("Types:\n\n");
        for m in types {
            md.push_str(&bullet(m));
            md.push('\n');
        }
        md.push('\n');
    }

    md.push_str("## Where it lives\n\n");
    let mut per_file: BTreeMap<&str, usize> = BTreeMap::new();
    for m in &members {
        if !m.file.is_empty() {
            *per_file.entry(m.file.as_str()).or_insert(0) += 1;
        }
    }
    let mut files: Vec<(&str, usize)> = per_file.into_iter().collect();
    files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    for (f, n) in files.iter().take(20) {
        let label = if root.join(f).is_file() {
            // Relative to docs/explanations/.
            format!("[{f}](../../{f})")
        } else {
            (*f).to_string()
        };
        md.push_str(&format!("- {label}: {n} symbol(s)\n"));
    }
    md.push('\n');

    let cited: Vec<&&ConceptSymbol> = ranked
        .iter()
        .filter(|m| !m.doc_comment.trim().is_empty())
        .take(5)
        .collect();
    if !cited.is_empty() {
        md.push_str("## Doc comments in the code\n\nVerbatim, with where they come from.\n\n");
        for m in cited {
            let text = defang(m.doc_comment.trim());
            let fence = fence_for(&text);
            md.push_str(&format!(
                "{}:{} ({})\n\n{fence}text\n{text}\n{fence}\n\n",
                m.file,
                m.line,
                short_name(&m.symbol)
            ));
        }
    }

    md.push_str(&format!(
        "## For the author\n\nThe code could not tell us these; add them before this page leaves draft:\n\n\
         - Why {name} exists and who depends on it.\n\
         - The invariants the code assumes but does not state.\n"
    ));

    let synonyms = if p.synonyms.is_empty() {
        String::new()
    } else {
        format!("synonyms: [{}]\n", p.synonyms.join(", "))
    };
    let mut modules = String::new();
    for f in p.source_files.iter().take(50) {
        modules.push_str(&format!("  - {f}\n"));
    }
    let entity_md = format!(
        "---\nid: entity-{name}\nrole: ontology-entity\ntitle: {etitle}\nsummary: {esummary}\n\
         status: stable\nconfidence: {conf:.2}\nupdated: {today}\naxis_id: covers\nvalue_id: {name}\n\
         display: {name}\ndescription: {desc}\n{synonyms}source_modules:\n{modules}---\n\n\
         # {name}\n\nAccepted from `doc-linter ontology propose`. Narrative: [[concept-{name}]].\n",
        etitle = yaml_str(&format!("Entity: {name}")),
        esummary = yaml_str(&format!("Concept {name}, accepted from the code graph")),
        conf = p.confidence,
        desc = yaml_str(&p.description),
    );
    Ok(Narrative {
        entity_md,
        explanation_md: md,
    })
}

pub(crate) fn accept(root: &Path, ontology: &Ontology, args: &AcceptArgs) -> Result<ExitCode> {
    let from = abs(root, &args.from);
    let text = std::fs::read_to_string(&from).with_context(|| {
        format!(
            "read {}: run `doc-linter ontology propose` first",
            from.display()
        )
    })?;
    let file: ProposalsFile =
        serde_json::from_str(&text).with_context(|| format!("parse {}", from.display()))?;

    let mut renames: HashMap<String, String> = HashMap::new();
    for r in &args.renames {
        let (old, new) = r
            .split_once('=')
            .ok_or_else(|| anyhow!("--rename expects old=new, got `{r}`"))?;
        renames.insert(old.trim().to_string(), new.trim().to_string());
    }

    let db = open_graph(root)?;
    let conn = &*db;
    let symbols = typed::concept_symbols(conn);
    let facts: HashMap<&str, &ConceptSymbol> =
        symbols.iter().map(|s| (s.symbol.as_str(), s)).collect();
    let calls = typed::concept_calls(conn);
    let existing = existing_names(ontology);
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let mut refused = 0usize;
    for requested in &args.names {
        let refuse = |why: &str| eprintln!("doc-linter: refused `{requested}`: {why}");
        let Some(p) = file.proposals.iter().find(|p| &p.name == requested) else {
            refuse(&format!("not in {}", from.display()));
            refused += 1;
            continue;
        };
        let name = renames.get(requested).unwrap_or(requested);
        if !valid_slug(name) {
            refuse(&format!(
                "`{name}` is not a valid concept name (lowercase letters, digits and hyphens, 3-40 chars)"
            ));
            refused += 1;
            continue;
        }
        let entity_path = root.join(format!("docs/ontology/entities/{name}.md"));
        let doc_path = root.join(format!("docs/explanations/concept-{name}.md"));
        if existing.contains(name.as_str()) || entity_path.exists() || doc_path.exists() {
            refuse(&format!(
                "`{name}` already exists (entity or explanation); nothing written"
            ));
            refused += 1;
            continue;
        }
        match render_concept(root, p, name, &facts, &calls, &today, ontology) {
            Err(why) => {
                refuse(&why);
                refused += 1;
            }
            Ok(n) => {
                if args.dry_run {
                    println!(
                        "would write {} and {}",
                        entity_path
                            .strip_prefix(root)
                            .unwrap_or(&entity_path)
                            .display(),
                        doc_path.strip_prefix(root).unwrap_or(&doc_path).display()
                    );
                    continue;
                }
                for (path, body) in [(&entity_path, &n.entity_md), (&doc_path, &n.explanation_md)] {
                    if let Some(dir) = path.parent() {
                        std::fs::create_dir_all(dir)
                            .with_context(|| format!("create {}", dir.display()))?;
                    }
                    std::fs::write(path, body)
                        .with_context(|| format!("write {}", path.display()))?;
                    println!(
                        "wrote {}",
                        path.strip_prefix(root).unwrap_or(path).display()
                    );
                }
            }
        }
    }
    Ok(if refused > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests panic on broken invariants -- that is the point of a test"
)]
mod tests {
    use super::*;

    fn sym(symbol: &str, file: &str, sig: &str, doc: &str, is_type: bool) -> ConceptSymbol {
        ConceptSymbol {
            symbol: symbol.to_string(),
            file: file.to_string(),
            line: 1,
            signature: sig.to_string(),
            doc_comment: doc.to_string(),
            is_type,
        }
    }

    fn pkg(path: &str) -> String {
        format!("rust-analyzer cargo demo 0.1.0 {path}")
    }

    fn args() -> ProposeArgs {
        ProposeArgs {
            top_n: 20,
            min_members: 3,
            algorithm: "leiden".into(),
            embeddings: false,
            json: false,
            out: PathBuf::from("x"),
        }
    }

    /// Test 1: TF-IDF names the billing cluster `invoice`/`billing`, and
    /// the generic words `new` and `get` never win.
    #[test]
    fn tfidf_names_billing_cluster_and_never_picks_generic_words() {
        let mut syms = Vec::new();
        for f in [
            "create_invoice",
            "compute_invoice_total",
            "apply_invoice_discount",
            "render_invoice_pdf",
        ] {
            syms.push(sym(
                &pkg(&format!("src/billing/invoice.rs/{f}().")),
                "src/billing/invoice.rs",
                &format!("fn {f}()"),
                "Handles the invoice for an order.",
                false,
            ));
        }
        for f in ["new", "get", "helper"] {
            syms.push(sym(
                &pkg(&format!("src/util.rs/{f}().")),
                "src/util.rs",
                "",
                "",
                false,
            ));
        }
        for f in ["open_session", "close_session", "touch_session"] {
            syms.push(sym(
                &pkg(&format!("src/session.rs/{f}().")),
                "src/session.rs",
                "",
                "",
                false,
            ));
        }
        let cfg = LintConfig::default();
        let stops = stop_list(&cfg);
        let props =
            build_proposals(syms, &[], &[], &stops, &HashSet::new(), None, &args()).unwrap();
        let names: Vec<&str> = props.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"invoice") || names.contains(&"billing"),
            "{names:?}"
        );
        assert!(!names.iter().any(|n| ["new", "get", "helper"].contains(n)));
        assert!(names.contains(&"session"), "{names:?}");
    }

    /// Test 2: a rare shared token joins call-disconnected symbols; a
    /// token every symbol has joins nothing.
    #[test]
    fn affinity_joins_on_rare_token_not_common_one() {
        let mut sets: Vec<BTreeSet<String>> = (0..30)
            .map(|i| {
                ["common".to_string(), format!("only{i}")]
                    .into_iter()
                    .collect()
            })
            .collect();
        sets[3].insert("zebra".into());
        sets[17].insert("zebra".into());
        let edges = affinity_edges(&sets, 8);
        assert_eq!(edges.into_iter().collect::<Vec<_>>(), vec![(3, 17)]);
    }

    /// Test 8 (unit half): stored vectors that are close join symbols that
    /// share no token and no call; without them nothing groups.
    #[test]
    fn embedding_edges_join_token_disjoint_symbols() {
        let syms: Vec<ConceptSymbol> = ["alpha", "bravo", "charlie"]
            .iter()
            .map(|n| {
                sym(
                    &pkg(&format!("src/{n}.rs/{n}().")),
                    &format!("src/{n}.rs"),
                    "fn x()",
                    "",
                    false,
                )
            })
            .collect();
        let vectors: Vec<(String, Vec<f32>)> = syms
            .iter()
            .map(|s| (s.symbol.clone(), vec![1.0, 0.0, 0.0]))
            .collect();
        let stops = stop_list(&LintConfig::default());
        let none = build_proposals(
            syms.clone(),
            &[],
            &[],
            &stops,
            &HashSet::new(),
            None,
            &args(),
        )
        .unwrap();
        assert!(none.is_empty(), "{none:?}");
        let some =
            build_proposals(syms, &[], &vectors, &stops, &HashSet::new(), None, &args()).unwrap();
        assert_eq!(some.len(), 1);
        assert_eq!(some[0].member_count, 3);
    }

    #[test]
    fn split_words_handles_camel_snake_and_acronyms() {
        assert_eq!(
            split_words("HTTPServer_parseURL"),
            ["http", "server", "parse", "url"]
        );
        assert_eq!(leaf_name("p src/a.rs/Invoice#total()."), "total");
        assert_eq!(leaf_name("p src/a.rs/create_invoice()."), "create_invoice");
    }

    #[test]
    fn thin_cluster_renders_nothing() {
        let syms = [
            sym("a", "x.rs", "", "", false),
            sym("b", "x.rs", "", "", false),
            sym("c", "x.rs", "fn c()", "", false),
        ];
        let facts: HashMap<&str, &ConceptSymbol> =
            syms.iter().map(|s| (s.symbol.as_str(), s)).collect();
        let p = Proposal {
            name: "zebra".into(),
            alt_names: vec![],
            confidence: 1.0,
            member_count: 3,
            namer: "tfidf".into(),
            description: "d".into(),
            synonyms: vec![],
            top_files: vec![],
            sample_doc_comment: String::new(),
            source_files: vec!["x.rs".into()],
            members: vec!["a".into(), "b".into(), "c".into()],
        };
        let ont = Ontology::bootstrap();
        let r = render_concept(
            Path::new("/nonexistent"),
            &p,
            "zebra",
            &facts,
            &[],
            "2026-01-01",
            &ont,
        );
        assert!(r.is_err());
    }
}
