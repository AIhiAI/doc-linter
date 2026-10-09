//! Test support: build an SQLite graph from the Cypher `CREATE` statements
//! the Kuzu-era tests used as a seeding notation, and read query results in
//! the shape those tests matched on. Test-only (`#[cfg(test)]`).
//!
//! Seeds: `CREATE (:Label {k: v, ...}), ...` and
//! `MATCH (a:Label {k: v}), (b:Label) WHERE b.k = 'x' CREATE (a)-[:REL {k: v}]->(b)`.
//! List properties (`tags`, `covers`, ...) go to their side tables, embedding
//! columns are ignored, an edge whose endpoint is missing inserts nothing
//! (as the Cypher `MATCH ... CREATE` did). Anything else is a test bug and
//! panics through `Result`.

#![allow(
    dead_code,
    unreachable_pub,
    clippy::type_complexity,
    clippy::format_collect,
    reason = "test-only shim: it mirrors the engine value type the old tests matched on"
)]

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};

use super::{schema, SqliteDb};
use crate::store::{Store, StoreRead, Value};

/// Value shim named after the engine type the old tests matched on
/// (`kuzu::Value::String(s)` becomes `kv::Value::String(s)`).
pub(crate) mod kv {
    #[derive(Debug, Clone, PartialEq)]
    pub enum Value {
        Null,
        Bool(bool),
        Int64(i64),
        Int32(i32),
        UInt32(u32),
        UInt64(u64),
        Int128(i128),
        Double(f64),
        Float(f32),
        String(String),
        List((), Vec<Value>),
    }
}

/// A fresh, empty graph (full schema) in `dir/.doc-lint/graph.sqlite`.
pub(crate) fn open_db(dir: &Path) -> SqliteDb {
    let _ = std::fs::remove_dir_all(dir.join(".doc-lint"));
    let db = super::open_rw(dir).expect("open sqlite graph");
    schema::create_schema(&db).expect("create schema");
    db
}

pub(crate) fn connect(db: &SqliteDb) -> Conn<'_> {
    Conn(db)
}

/// The old `reset_schema_with_mode`: a fresh [`open_db`] is already empty.
pub(crate) fn reset(_: &Conn<'_>) {}

/// Run a built-in saved query (same entry point the CLI uses).
pub(crate) fn run_saved_query(
    db: &SqliteDb,
    name: &str,
    params: &HashMap<String, String>,
) -> Result<serde_json::Value> {
    super::saved::run_saved_query_with_root(db, None, name, params)
}

/// SQL text of a built-in saved query.
pub(crate) fn sql_of(name: &str) -> &'static str {
    super::saved::builtin_sql(name).unwrap_or_else(|| panic!("no SQL for saved query {name}"))
}

pub(crate) struct Conn<'a>(&'a SqliteDb);

pub(crate) struct QueryResult(std::vec::IntoIter<Vec<kv::Value>>);

impl Iterator for QueryResult {
    type Item = Vec<kv::Value>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

impl QueryResult {
    pub(crate) fn get_num_tuples(&self) -> u64 {
        self.0.len() as u64
    }
}

impl Conn<'_> {
    /// A seed (`CREATE ...` / `MATCH ... CREATE ...`) or a read-only SQL
    /// query, dispatched on the first keyword.
    pub(crate) fn query(&self, text: &str) -> Result<QueryResult> {
        let head = text
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        if head == "CREATE" || head == "MATCH" {
            seed(self.0, text)?;
            return Ok(QueryResult(Vec::new().into_iter()));
        }
        let json = text.contains("json_group_array") || text.contains("json_array");
        let rows = StoreRead::query(self.0, text, &[])?;
        Ok(QueryResult(
            rows.into_iter()
                .map(|r| r.into_iter().map(|v| to_kv(v, json)).collect::<Vec<_>>())
                .collect::<Vec<_>>()
                .into_iter(),
        ))
    }
}

fn to_kv(v: Value, json: bool) -> kv::Value {
    match v {
        Value::Null => kv::Value::Null,
        Value::Bool(b) => kv::Value::Bool(b),
        Value::Int(n) => kv::Value::Int64(n),
        Value::Float(f) => kv::Value::Double(f),
        Value::Str(s) => {
            if json && s.starts_with('[') {
                if let Ok(serde_json::Value::Array(a)) = serde_json::from_str(&s) {
                    return kv::Value::List((), a.into_iter().map(json_kv).collect());
                }
            }
            kv::Value::String(s)
        }
        Value::List(xs) => kv::Value::List((), xs.into_iter().map(|x| to_kv(x, false)).collect()),
    }
}

fn json_kv(j: serde_json::Value) -> kv::Value {
    match j {
        serde_json::Value::Null => kv::Value::Null,
        serde_json::Value::Bool(b) => kv::Value::Bool(b),
        serde_json::Value::Number(n) => n.as_i64().map_or_else(
            || kv::Value::Double(n.as_f64().unwrap_or(0.0)),
            kv::Value::Int64,
        ),
        serde_json::Value::String(s) => kv::Value::String(s),
        other => kv::Value::String(other.to_string()),
    }
}

// --- Cypher CREATE subset ------------------------------------------------

#[derive(Debug, Clone)]
enum Lit {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Lit>),
}

impl Lit {
    fn value(&self) -> Value {
        match self {
            Lit::Null => Value::Null,
            Lit::Bool(b) => Value::Int(i64::from(*b)),
            Lit::Int(n) => Value::Int(*n),
            Lit::Float(f) => Value::Float(*f),
            Lit::Str(s) => Value::Str(s.clone()),
            Lit::List(_) => Value::Null,
        }
    }
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }
    fn eat(&mut self, t: &str) -> bool {
        self.ws();
        if self.s[self.i..].starts_with(t.as_bytes()) {
            self.i += t.len();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, t: &str) -> Result<()> {
        if self.eat(t) {
            Ok(())
        } else {
            bail!("seed: expected `{t}` at byte {}", self.i)
        }
    }
    /// Case-insensitive keyword that must end at a word boundary.
    fn kw(&mut self, k: &str) -> bool {
        self.ws();
        let end = self.i + k.len();
        let ok = self.s.len() >= end
            && self.s[self.i..end].eq_ignore_ascii_case(k.as_bytes())
            && !self
                .s
                .get(end)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_');
        if ok {
            self.i = end;
        }
        ok
    }
    fn ident(&mut self) -> Result<String> {
        self.ws();
        let st = self.i;
        while self
            .s
            .get(self.i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            self.i += 1;
        }
        if st == self.i {
            bail!("seed: identifier expected at byte {st}");
        }
        Ok(String::from_utf8_lossy(&self.s[st..self.i]).into_owned())
    }
    fn lit(&mut self) -> Result<Lit> {
        self.ws();
        match self.s.get(self.i) {
            Some(b'\'') => {
                self.i += 1;
                let mut out = Vec::new();
                loop {
                    match self.s.get(self.i) {
                        None => bail!("seed: unterminated string"),
                        Some(b'\\') => {
                            out.push(*self.s.get(self.i + 1).context("seed: dangling escape")?);
                            self.i += 2;
                        }
                        Some(b'\'') => {
                            self.i += 1;
                            break;
                        }
                        Some(c) => {
                            out.push(*c);
                            self.i += 1;
                        }
                    }
                }
                Ok(Lit::Str(String::from_utf8_lossy(&out).into_owned()))
            }
            Some(b'[') => {
                self.i += 1;
                let mut xs = Vec::new();
                if self.eat("]") {
                    return Ok(Lit::List(xs));
                }
                loop {
                    xs.push(self.lit()?);
                    if self.eat(",") {
                        continue;
                    }
                    self.expect("]")?;
                    return Ok(Lit::List(xs));
                }
            }
            Some(c) if c.is_ascii_digit() || *c == b'-' => {
                let st = self.i;
                self.i += 1;
                while self
                    .s
                    .get(self.i)
                    .is_some_and(|c| c.is_ascii_digit() || *c == b'.' || *c == b'e')
                {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[st..self.i])?;
                Ok(t.parse::<i64>()
                    .map(Lit::Int)
                    .or_else(|_| t.parse::<f64>().map(Lit::Float))?)
            }
            _ => {
                let w = self.ident()?;
                match w.to_ascii_lowercase().as_str() {
                    "true" => Ok(Lit::Bool(true)),
                    "false" => Ok(Lit::Bool(false)),
                    "null" => Ok(Lit::Null),
                    _ => bail!("seed: unsupported value `{w}`"),
                }
            }
        }
    }
    fn props(&mut self) -> Result<Vec<(String, Lit)>> {
        let mut out = Vec::new();
        if !self.eat("{") {
            return Ok(out);
        }
        if self.eat("}") {
            return Ok(out);
        }
        loop {
            let k = self.ident()?;
            self.expect(":")?;
            out.push((k, self.lit()?));
            if self.eat(",") {
                continue;
            }
            self.expect("}")?;
            return Ok(out);
        }
    }
    /// `(var:Label {props})`, each part optional.
    fn node(&mut self) -> Result<(String, String, Vec<(String, Lit)>)> {
        self.expect("(")?;
        self.ws();
        let var = if matches!(self.s.get(self.i), Some(b':' | b')')) {
            String::new()
        } else {
            self.ident()?
        };
        let label = if self.eat(":") {
            self.ident()?
        } else {
            String::new()
        };
        let props = self.props()?;
        self.expect(")")?;
        Ok((var, label, props))
    }
}

const PK: &[(&str, &str)] = &[
    ("Doc", "id"),
    ("Entity", "id"),
    ("Section", "id"),
    ("Function", "symbol"),
    ("Type", "symbol"),
    ("Field", "symbol"),
    ("File", "path"),
    ("Module", "id"),
    ("Finding", "id"),
    ("Migration", "id"),
    ("RepoMeta", "id"),
    ("Repo", "id"),
    ("Endpoint", "id"),
];

fn pk_of(label: &str) -> Result<&'static str> {
    PK.iter()
        .find(|(l, _)| *l == label)
        .map(|(_, c)| *c)
        .with_context(|| format!("seed: unknown label `{label}`"))
}

/// Side table and owner column for a list property, e.g. `Doc.tags`.
fn list_table(label: &str, prop: &str) -> Option<(String, &'static str)> {
    let owner = match label {
        "Doc" => ("doc", "doc_id"),
        "Entity" => ("entity", "entity_id"),
        _ => return None,
    };
    Some((format!("{}_{prop}", owner.0), owner.1))
}

fn columns(db: &SqliteDb, table: &str) -> Result<Vec<String>> {
    Ok(StoreRead::query(
        db,
        &format!("SELECT name FROM pragma_table_info('{table}')"),
        &[],
    )?
    .iter()
    .map(|r| r[0].str_or_empty())
    .collect())
}

fn seed(db: &SqliteDb, text: &str) -> Result<()> {
    let mut p = P {
        s: text.as_bytes(),
        i: 0,
    };
    if p.kw("CREATE") {
        loop {
            let (_, label, props) = p.node()?;
            insert_node(db, &label, &props)?;
            if !p.eat(",") {
                break;
            }
        }
        return Ok(());
    }
    if !p.kw("MATCH") {
        bail!("seed: expected CREATE or MATCH: {text}");
    }
    let mut nodes: Vec<(String, String, Vec<(String, Lit)>)> = Vec::new();
    loop {
        nodes.push(p.node()?);
        if !p.eat(",") {
            break;
        }
    }
    if p.kw("WHERE") {
        loop {
            let var = p.ident()?;
            p.expect(".")?;
            let col = p.ident()?;
            p.expect("=")?;
            let v = p.lit()?;
            nodes
                .iter_mut()
                .find(|n| n.0 == var)
                .with_context(|| format!("seed: unknown variable `{var}`"))?
                .2
                .push((col, v));
            if !p.kw("AND") {
                break;
            }
        }
    }
    if !p.kw("CREATE") {
        bail!("seed: MATCH without CREATE: {text}");
    }
    let (from, ..) = p.node()?;
    let forward = if p.eat("-[") {
        true
    } else {
        p.expect("<-[")?;
        false
    };
    p.expect(":")?;
    let rel = p.ident()?;
    let props = p.props()?;
    p.expect("]")?;
    p.expect(if forward { "->" } else { "-" })?;
    let (to, ..) = p.node()?;
    let (a, b) = if forward { (from, to) } else { (to, from) };
    let (mut cols, mut vals) = (Vec::new(), Vec::<(String, Value)>::new());
    for (i, (k, v)) in props.iter().enumerate() {
        cols.push(k.clone());
        vals.push((format!("e{i}"), v.value()));
    }
    let find = |var: &str| -> Result<usize> {
        nodes
            .iter()
            .position(|n| n.0 == var)
            .with_context(|| format!("seed: unknown variable `{var}`"))
    };
    let (ia, ib) = (find(&a)?, find(&b)?);
    let mut wheres = Vec::new();
    for (n, (_, label, conds)) in nodes.iter().enumerate() {
        pk_of(label)?;
        for (j, (col, lit)) in conds.iter().enumerate() {
            let name = format!("w{n}_{j}");
            wheres.push(format!("n{n}.{col} = ${name}"));
            vals.push((name, lit.value()));
        }
    }
    let from_clause = nodes
        .iter()
        .enumerate()
        .map(|(n, (_, label, _))| format!("{label} n{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO \"{rel}\"(src, dst{}) SELECT n{ia}.{}, n{ib}.{}{} FROM {from_clause}{}",
        cols.iter().map(|c| format!(", {c}")).collect::<String>(),
        pk_of(&nodes[ia].1)?,
        pk_of(&nodes[ib].1)?,
        (0..cols.len())
            .map(|i| format!(", $e{i}"))
            .collect::<String>(),
        if wheres.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", wheres.join(" AND "))
        }
    );
    let params: Vec<(&str, Value)> = vals.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    db.exec(&sql, &params).with_context(|| sql.clone())
}

fn insert_node(db: &SqliteDb, label: &str, props: &[(String, Lit)]) -> Result<()> {
    let pk = pk_of(label)?;
    let have = columns(db, label)?;
    let mut cols = Vec::new();
    let mut vals: Vec<(String, Value)> = Vec::new();
    let mut lists: Vec<(String, &'static str, Vec<Lit>)> = Vec::new();
    for (k, v) in props {
        if let Lit::List(xs) = v {
            if let Some((t, owner)) = list_table(label, k) {
                lists.push((t, owner, xs.clone()));
            }
            continue;
        }
        if k.ends_with("_embedding") {
            continue;
        }
        if !have.contains(k) {
            bail!("seed: {label} has no column `{k}`");
        }
        cols.push(k.clone());
        vals.push((format!("v{}", vals.len()), v.value()));
    }
    let sql = format!(
        "INSERT INTO {label}({}) VALUES ({})",
        cols.join(", "),
        (0..cols.len())
            .map(|i| format!("$v{i}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let params: Vec<(&str, Value)> = vals.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    db.exec(&sql, &params).with_context(|| sql.clone())?;
    let id = props
        .iter()
        .find(|(k, _)| k == pk)
        .map(|(_, v)| v.value())
        .with_context(|| format!("seed: {label} needs `{pk}`"))?;
    for (t, owner, xs) in lists {
        for x in xs {
            db.exec(
                &format!("INSERT INTO {t}({owner}, value) VALUES ($o, $v)"),
                &[("o", id.clone()), ("v", x.value())],
            )?;
        }
    }
    Ok(())
}
