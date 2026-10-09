//! Storage seam (docs/design/store-trait.md): an engine-neutral `Value` and
//! the `StoreRead` / `Store` traits. The SQLite impl lives in
//! `store_sqlite`.

use anyhow::Result;

/// Engine-neutral cell / parameter value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Integer cell, `None` for anything else.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// Non-negative integer cell; every other shape reads as 0.
    pub fn as_u64_or_zero(&self) -> u64 {
        self.as_i64().map_or(0, |n| n as u64)
    }

    /// String cell, empty for anything else.
    pub fn str_or_empty(&self) -> String {
        self.as_str().unwrap_or_default().to_string()
    }
}

pub type Row = Vec<Value>;
pub type Params<'a> = &'a [(&'a str, Value)];

pub trait StoreRead {
    /// Run one statement with `$name` bound parameters; rows come back
    /// eagerly collected.
    fn query(&self, stmt: &str, params: Params) -> Result<Vec<Row>>;
}

pub trait Store: StoreRead {
    fn exec(&self, stmt: &str, params: Params) -> Result<()>;
}
