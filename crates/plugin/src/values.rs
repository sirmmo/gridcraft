//! Cell values on the wire.
//!
//! | Cell value | JSON |
//! |---|---|
//! | empty | `null` |
//! | number | number |
//! | text | string |
//! | logical | `true` / `false` |
//! | error | `{"error": "#N/A"}` |
//! | array (a range) | rows of values: `[[1, 2], [3, 4]]` |
//!
//! A plugin's result may also be a flat list, read as one row. Ragged rows are padded with
//! `#N/A`, as Excel pads arrays.

use std::sync::Arc;

use gridcraft_core::{Array, CellError, Value};
use serde_json::{Value as Json, json};

/// Most cells sent to or accepted from a plugin in one call.
pub const MAX_CELLS: usize = 1_000_000;
/// Excel's cell text limit.
const MAX_TEXT: usize = 32_767;

pub fn to_json(v: &Value) -> Json {
    match v {
        Value::Empty => Json::Null,
        Value::Number(n) => serde_json::Number::from_f64(*n).map_or(Json::Null, Json::Number),
        Value::Text(t) => Json::String(t.to_string()),
        Value::Bool(b) => Json::Bool(*b),
        Value::Error(e) => json!({"error": e.as_str()}),
        Value::Array(a) => {
            let cols = a.cols.max(1);
            Json::Array(a.data.chunks(cols).map(|row| Json::Array(row.iter().map(to_json).collect())).collect())
        }
    }
}

/// A plugin's result as a cell value; anything malformed is `#VALUE!`.
pub fn from_json(j: &Json) -> Value {
    match j {
        Json::Array(rows) => array(rows),
        j => scalar(j),
    }
}

fn scalar(j: &Json) -> Value {
    match j {
        Json::Null => Value::Empty,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => n.as_f64().map_or(Value::Error(CellError::Num), Value::number),
        Json::String(s) => Value::Text(Arc::from(truncate(s))),
        Json::Object(o) => match o.get("error").and_then(Json::as_str) {
            Some(e) => Value::Error(CellError::parse(e).unwrap_or(CellError::Value)),
            None => Value::Error(CellError::Value),
        },
        Json::Array(_) => Value::Error(CellError::Value),
    }
}

fn truncate(s: &str) -> &str {
    match s.char_indices().nth(MAX_TEXT) {
        Some((i, _)) => s.get(..i).unwrap_or(s),
        None => s,
    }
}

fn array(rows: &[Json]) -> Value {
    if rows.is_empty() {
        return Value::Error(CellError::Calc);
    }
    // A flat list is one row.
    if !rows.iter().any(Json::is_array) {
        if rows.len() > MAX_CELLS {
            return Value::Error(CellError::Num);
        }
        return Value::from(Array::row(rows.iter().map(scalar).collect()));
    }
    let width = rows.iter().map(|r| r.as_array().map_or(1, Vec::len)).max().unwrap_or(1);
    if width == 0 {
        return Value::Error(CellError::Calc);
    }
    if rows.len().checked_mul(width).is_none_or(|n| n > MAX_CELLS) {
        return Value::Error(CellError::Num);
    }
    let mut data = Vec::with_capacity(rows.len() * width);
    for r in rows {
        let row: &[Json] = match r {
            Json::Array(cells) => cells,
            single => std::slice::from_ref(single),
        };
        data.extend(row.iter().map(scalar));
        data.extend(std::iter::repeat_n(Value::Error(CellError::NA), width - row.len()));
    }
    match Array::new(rows.len(), width, data) {
        Some(a) => Value::from(a),
        None => Value::Error(CellError::Value),
    }
}

/// Cells in a list of arguments (an array counts each of its cells).
pub fn cell_count(args: &[Value]) -> usize {
    args.iter().map(|a| if let Value::Array(x) = a { x.data.len() } else { 1 }).fold(0usize, usize::saturating_add)
}
