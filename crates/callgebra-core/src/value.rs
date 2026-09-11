//! Scalar values, data types, fields and schemas.

use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

/// The type of a column or a function argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataType {
    /// Accepts any value; used for untyped literals and `NULL`.
    Any,
    /// `BOOLEAN`.
    Bool,
    /// 64-bit signed `INTEGER` / `BIGINT`.
    Int,
    /// 64-bit `DOUBLE`.
    Float,
    /// `TEXT`.
    Text,
    /// `JSON`, stored as a parsed document.
    Json,
    /// Embedding vector of `f32`.
    Vector,
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            DataType::Any => "ANY",
            DataType::Bool => "BOOLEAN",
            DataType::Int => "BIGINT",
            DataType::Float => "DOUBLE",
            DataType::Text => "TEXT",
            DataType::Json => "JSON",
            DataType::Vector => "VECTOR",
        };
        f.write_str(s)
    }
}

/// A scalar value.
///
/// `Eq`, `Ord` and `Hash` are total so values can be deduplicated and used as
/// memo keys: floats compare with [`f64::total_cmp`] and `NULL` sorts first.
/// This is *not* SQL three-valued comparison; the executor implements that on
/// top with [`Value::sql_eq`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    /// SQL `NULL`.
    Null,
    /// Boolean.
    Bool(bool),
    /// 64-bit integer.
    Int(i64),
    /// 64-bit float.
    Float(f64),
    /// UTF-8 text.
    Text(String),
    /// JSON document.
    Json(serde_json::Value),
    /// Embedding vector.
    Vector(Vec<f32>),
}

impl Value {
    /// The data type of this value; `NULL` is [`DataType::Any`].
    pub fn data_type(&self) -> DataType {
        match self {
            Value::Null => DataType::Any,
            Value::Bool(_) => DataType::Bool,
            Value::Int(_) => DataType::Int,
            Value::Float(_) => DataType::Float,
            Value::Text(_) => DataType::Text,
            Value::Json(_) => DataType::Json,
            Value::Vector(_) => DataType::Vector,
        }
    }

    /// `true` for SQL `NULL`.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// SQL equality with three-valued logic: `NULL` if either side is `NULL`.
    /// Integers and floats compare numerically.
    pub fn sql_eq(&self, other: &Value) -> Option<bool> {
        match (self, other) {
            (Value::Null, _) | (_, Value::Null) => None,
            (Value::Int(a), Value::Float(b)) => Some((*a as f64) == *b),
            (Value::Float(a), Value::Int(b)) => Some(*a == (*b as f64)),
            (a, b) => Some(a == b),
        }
    }

    /// SQL ordering with three-valued logic: `None` if either side is `NULL`
    /// or the types are not comparable.
    pub fn sql_cmp(&self, other: &Value) -> Option<Ordering> {
        match (self, other) {
            (Value::Null, _) | (_, Value::Null) => None,
            (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
            (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
            (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
            (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
            (Value::Text(a), Value::Text(b)) => Some(a.cmp(b)),
            _ => None,
        }
    }

    /// Text view for rendering and prompts. `NULL` renders as `NULL`.
    pub fn render(&self) -> String {
        match self {
            Value::Null => "NULL".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Text(s) => s.clone(),
            Value::Json(j) => j.to_string(),
            Value::Vector(v) => format!("[{} floats]", v.len()),
        }
    }

    /// Borrow as text, if this is a `TEXT`.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    /// Borrow as bool, if this is a `BOOLEAN`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Borrow as integer, if this is an `INTEGER`.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// Numeric view: integers widen to float.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Cast to a target type using SQL-like rules. `NULL` casts to `NULL`.
    pub fn cast(&self, to: DataType) -> Result<Value, CoreError> {
        let err = || CoreError::Cast {
            from: self.type_name(),
            to: to.to_string(),
        };
        if self.is_null() || to == DataType::Any {
            return Ok(self.clone());
        }
        Ok(match (self, to) {
            (v, t) if v.data_type() == t => v.clone(),
            (Value::Int(i), DataType::Float) => Value::Float(*i as f64),
            (Value::Float(f), DataType::Int) if f.fract() == 0.0 => Value::Int(*f as i64),
            (Value::Bool(b), DataType::Int) => Value::Int(i64::from(*b)),
            (Value::Int(i), DataType::Bool) => Value::Bool(*i != 0),
            (Value::Text(s), DataType::Int) => Value::Int(s.trim().parse().map_err(|_| err())?),
            (Value::Text(s), DataType::Float) => Value::Float(s.trim().parse().map_err(|_| err())?),
            (Value::Text(s), DataType::Bool) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "t" | "yes" | "1" => Value::Bool(true),
                "false" | "f" | "no" | "0" => Value::Bool(false),
                _ => return Err(err()),
            },
            (Value::Text(s), DataType::Json) => {
                Value::Json(serde_json::from_str(s).map_err(|_| err())?)
            }
            (v, DataType::Text) => Value::Text(v.render()),
            (Value::Json(j), DataType::Vector) => {
                let arr = j.as_array().ok_or_else(err)?;
                let mut out = Vec::with_capacity(arr.len());
                for x in arr {
                    out.push(x.as_f64().ok_or_else(err)? as f32);
                }
                Value::Vector(out)
            }
            _ => return Err(err()),
        })
    }

    fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "NULL",
            Value::Bool(_) => "BOOLEAN",
            Value::Int(_) => "BIGINT",
            Value::Float(_) => "DOUBLE",
            Value::Text(_) => "TEXT",
            Value::Json(_) => "JSON",
            Value::Vector(_) => "VECTOR",
        }
    }

    fn rank(&self) -> u8 {
        match self {
            Value::Null => 0,
            Value::Bool(_) => 1,
            Value::Int(_) => 2,
            Value::Float(_) => 3,
            Value::Text(_) => 4,
            Value::Json(_) => 5,
            Value::Vector(_) => 6,
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Value {}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Value {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
            (Value::Int(a), Value::Int(b)) => a.cmp(b),
            (Value::Float(a), Value::Float(b)) => a.total_cmp(b),
            (Value::Text(a), Value::Text(b)) => a.cmp(b),
            (Value::Json(a), Value::Json(b)) => a.to_string().cmp(&b.to_string()),
            (Value::Vector(a), Value::Vector(b)) => {
                let by_elem = a.iter().zip(b).map(|(x, y)| x.total_cmp(y));
                for o in by_elem {
                    if o != Ordering::Equal {
                        return o;
                    }
                }
                a.len().cmp(&b.len())
            }
            (a, b) => a.rank().cmp(&b.rank()),
        }
    }
}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.rank().hash(state);
        match self {
            Value::Null => {}
            Value::Bool(b) => b.hash(state),
            Value::Int(i) => i.hash(state),
            Value::Float(f) => f.to_bits().hash(state),
            Value::Text(s) => s.hash(state),
            Value::Json(j) => j.to_string().hash(state),
            Value::Vector(v) => {
                for x in v {
                    x.to_bits().hash(state);
                }
            }
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Int(i64::from(v))
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(v.to_string())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Text(v)
    }
}
impl From<serde_json::Value> for Value {
    fn from(v: serde_json::Value) -> Self {
        Value::Json(v)
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        v.map(Into::into).unwrap_or(Value::Null)
    }
}

/// A named, typed column.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Field {
    /// Column name as the model sees it.
    pub name: String,
    /// Declared type.
    pub data_type: DataType,
    /// Whether `NULL` may appear.
    pub nullable: bool,
}

impl Field {
    /// A nullable field.
    pub fn new(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            name: name.into(),
            data_type,
            nullable: true,
        }
    }

    /// A field that never holds `NULL`.
    pub fn not_null(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            name: name.into(),
            data_type,
            nullable: false,
        }
    }
}

/// An ordered list of fields.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Schema {
    /// Fields in column order.
    pub fields: Vec<Field>,
}

impl Schema {
    /// Build a schema from fields.
    pub fn new(fields: Vec<Field>) -> Self {
        Self { fields }
    }

    /// The empty schema, for statements that return no rows.
    pub fn empty() -> Self {
        Self { fields: vec![] }
    }

    /// Number of columns.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// `true` if there are no columns.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Position of a column by name (case-insensitive, as in SQL).
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.fields
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(name))
    }

    /// Position of a column by name, or an error naming it.
    pub fn require(&self, name: &str) -> Result<usize, CoreError> {
        self.index_of(name)
            .ok_or_else(|| CoreError::NoSuchColumn(name.to_string()))
    }

    /// Column names in order.
    pub fn names(&self) -> Vec<&str> {
        self.fields.iter().map(|f| f.name.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_order_puts_null_first_and_is_stable() {
        let mut vs = [
            Value::from("b"),
            Value::from(2.5),
            Value::Null,
            Value::from(1),
            Value::from(true),
        ];
        vs.sort();
        assert_eq!(vs[0], Value::Null);
        assert_eq!(vs[1], Value::Bool(true));
        assert_eq!(vs[2], Value::Int(1));
    }

    #[test]
    fn sql_eq_is_three_valued_and_numeric() {
        assert_eq!(Value::Null.sql_eq(&Value::Int(1)), None);
        assert_eq!(Value::Int(1).sql_eq(&Value::Float(1.0)), Some(true));
        assert_eq!(Value::from("a").sql_eq(&Value::from("b")), Some(false));
    }

    #[test]
    fn casts_follow_sql_rules() {
        assert_eq!(
            Value::from(" 42 ").cast(DataType::Int).unwrap(),
            Value::Int(42)
        );
        assert_eq!(
            Value::Int(3).cast(DataType::Text).unwrap(),
            Value::from("3")
        );
        assert!(Value::from("x").cast(DataType::Int).is_err());
        assert_eq!(Value::Null.cast(DataType::Int).unwrap(), Value::Null);
    }

    #[test]
    fn schema_lookup_is_case_insensitive() {
        let s = Schema::new(vec![Field::new("Path", DataType::Text)]);
        assert_eq!(s.index_of("path"), Some(0));
        assert!(s.require("nope").is_err());
    }
}
