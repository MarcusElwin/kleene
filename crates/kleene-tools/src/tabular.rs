//! CSV and JSON files as relations: the parsing behind the `read_csv` and
//! `read_json` table functions and `kleene run --load`.
//!
//! A file becomes a list of records (JSON objects keyed by column), and a
//! list of records becomes a typed [`Batch`]: a column whose every
//! non-empty value is an integer is `BIGINT`, numeric is `DOUBLE`,
//! `true`/`false` is `BOOLEAN`, a nested object or array is `JSON`, and
//! anything else is `TEXT`; an empty CSV field is `NULL`.

use crate::ToolError;
use kleene_core::{Batch, DataType, Field, Schema, Value};
use serde_json::{Map, Value as Json};
use std::sync::Arc;

/// Parse RFC 4180 CSV: the first line names the columns; fields are split on
/// `,`, quoted with `"` (doubled inside), and a quoted field may span lines.
/// CRLF and LF both end a line; a byte-order mark is dropped.
pub fn parse_csv(text: &str) -> Result<(Vec<String>, Vec<Vec<String>>), ToolError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows: Vec<Vec<String>> = vec![];
    let mut row: Vec<String> = vec![];
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let mut had_quote = false;
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                other => field.push(other),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() && !had_quote => {
                quoted = true;
                had_quote = true;
            }
            ',' => {
                row.push(std::mem::take(&mut field));
                had_quote = false;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                had_quote = false;
                rows.push(std::mem::take(&mut row));
            }
            other => field.push(other),
        }
    }
    if quoted {
        return Err(ToolError::Args(
            "unterminated quoted field at the end of the CSV".into(),
        ));
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    let mut rows = rows.into_iter();
    let Some(header) = rows.next() else {
        return Err(ToolError::Args("the CSV is empty: no header line".into()));
    };
    let header: Vec<String> = header
        .into_iter()
        .enumerate()
        .map(|(i, h)| {
            let h = h.trim();
            if h.is_empty() {
                format!("column{}", i + 1)
            } else {
                h.to_string()
            }
        })
        .collect();
    let body: Vec<Vec<String>> = rows
        .enumerate()
        .filter(|(_, r)| !(r.len() == 1 && r[0].trim().is_empty()))
        .map(|(i, mut r)| {
            if r.len() > header.len() {
                return Err(ToolError::Args(format!(
                    "line {} has {} fields but the header names {} columns",
                    i + 2,
                    r.len(),
                    header.len()
                )));
            }
            r.resize(header.len(), String::new());
            Ok(r)
        })
        .collect::<Result<_, _>>()?;
    Ok((header, body))
}

/// CSV text as records: one object per line, keyed by the header, every
/// value a string (an empty field is omitted, which loads as `NULL`).
pub fn csv_records(text: &str) -> Result<Vec<Json>, ToolError> {
    let (header, rows) = parse_csv(text)?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let mut obj = Map::new();
            for (h, v) in header.iter().zip(r) {
                if !v.is_empty() {
                    obj.insert(h.clone(), Json::String(v));
                }
            }
            Json::Object(obj)
        })
        .collect())
}

/// JSON text as records: an array (one record per element), an object with
/// one array value (its elements), a single object (one record), or JSON
/// Lines (one value per line).
pub fn json_records(text: &str) -> Result<Vec<Json>, ToolError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).trim();
    if text.is_empty() {
        return Ok(vec![]);
    }
    match serde_json::from_str::<Json>(text) {
        Ok(Json::Array(items)) => Ok(items),
        Ok(Json::Object(obj)) => {
            let arrays: Vec<&Json> = obj.values().filter(|v| v.is_array()).collect();
            if let [only] = arrays.as_slice() {
                Ok(only.as_array().cloned().unwrap_or_default())
            } else {
                Ok(vec![Json::Object(obj)])
            }
        }
        Ok(other) => Ok(vec![other]),
        Err(e) => {
            // JSON Lines: every non-blank line is one value.
            let mut out = vec![];
            for (i, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                out.push(serde_json::from_str::<Json>(line).map_err(|_| {
                    ToolError::Args(format!(
                        "not JSON (an array, an object or one value per line): {e} (line {})",
                        i + 1
                    ))
                })?);
            }
            Ok(out)
        }
    }
}

/// Records as a typed batch: the columns are `order` first, then any other
/// keys as they appear (a JSON object's keys come sorted, so a JSON file's
/// columns are alphabetical unless `order` says otherwise; a non-object
/// record goes in a single column `value`), the types as described in the
/// module docs.
pub fn batch_from_records(records: &[Json], order: &[String]) -> Result<Batch, ToolError> {
    let mut columns: Vec<String> = order.to_vec();
    for r in records {
        match r {
            Json::Object(o) => {
                for k in o.keys() {
                    if !columns.contains(k) {
                        columns.push(k.clone());
                    }
                }
            }
            _ => {
                if !columns.iter().any(|c| c == "value") {
                    columns.push("value".into());
                }
            }
        }
    }
    let cell = |r: &Json, col: &str| -> Json {
        match r {
            Json::Object(o) => o.get(col).cloned().unwrap_or(Json::Null),
            other if col == "value" => other.clone(),
            _ => Json::Null,
        }
    };
    let types: Vec<DataType> = columns
        .iter()
        .map(|c| infer(records.iter().map(|r| cell(r, c))))
        .collect();
    let schema = Arc::new(Schema::new(
        columns
            .iter()
            .zip(&types)
            .map(|(c, t)| Field::new(c.clone(), *t))
            .collect(),
    ));
    let rows: Vec<Vec<Value>> = records
        .iter()
        .map(|r| {
            columns
                .iter()
                .zip(&types)
                .map(|(c, t)| coerce(cell(r, c), *t))
                .collect()
        })
        .collect();
    Batch::try_new(schema, rows).map_err(|e| ToolError::Args(e.to_string()))
}

/// The narrowest type every non-null value of a column fits.
fn infer(values: impl Iterator<Item = Json>) -> DataType {
    let mut seen = false;
    let mut all_int = true;
    let mut all_num = true;
    let mut all_bool = true;
    let mut all_json = true;
    for v in values {
        let s = match &v {
            Json::Null => continue,
            Json::String(s) if s.trim().is_empty() => continue,
            Json::String(s) => s.trim().to_string(),
            Json::Bool(b) => b.to_string(),
            Json::Number(n) => n.to_string(),
            Json::Array(_) | Json::Object(_) => {
                seen = true;
                all_int = false;
                all_num = false;
                all_bool = false;
                continue;
            }
        };
        seen = true;
        all_json = false;
        all_int &= s.parse::<i64>().is_ok();
        all_num &= s.parse::<f64>().is_ok();
        all_bool &= matches!(s.to_ascii_lowercase().as_str(), "true" | "false");
    }
    match (seen, all_int, all_num, all_bool, all_json) {
        (false, ..) => DataType::Text,
        (_, true, ..) => DataType::Int,
        (_, _, true, ..) => DataType::Float,
        (_, _, _, true, _) => DataType::Bool,
        (_, _, _, _, true) => DataType::Json,
        _ => DataType::Text,
    }
}

fn coerce(v: Json, t: DataType) -> Value {
    let s = match v {
        Json::Null => return Value::Null,
        Json::String(s) if s.trim().is_empty() => return Value::Null,
        Json::Array(_) | Json::Object(_) => return Value::Json(v),
        Json::String(s) => s,
        Json::Bool(b) => b.to_string(),
        Json::Number(n) => n.to_string(),
    };
    match t {
        DataType::Int => s.trim().parse().map(Value::Int).unwrap_or(Value::Text(s)),
        DataType::Float => s.trim().parse().map(Value::Float).unwrap_or(Value::Text(s)),
        DataType::Bool => Value::Bool(s.trim().eq_ignore_ascii_case("true")),
        DataType::Json => Value::Json(Json::String(s)),
        _ => Value::Text(s),
    }
}

/// A CSV or JSON file (by extension, `.csv` else JSON) as a typed batch.
pub fn batch_from_file(path: &std::path::Path) -> Result<Batch, ToolError> {
    let bytes = std::fs::read(path)?;
    let text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => {
            if let Some(rest) = e.as_bytes().strip_prefix(&[0xFF, 0xFE]) {
                let units: Vec<u16> = rest
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .collect();
                String::from_utf16(&units)
                    .map_err(|e| ToolError::Args(format!("{}: {e}", path.display())))?
            } else {
                return Err(ToolError::Args(format!(
                    "{}: not UTF-8 at byte {}; save the file as UTF-8",
                    path.display(),
                    e.utf8_error().valid_up_to()
                )));
            }
        }
    };
    let csv = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("csv"));
    if csv {
        let (header, _) = parse_csv(&text)?;
        batch_from_records(&csv_records(&text)?, &header)
    } else {
        batch_from_records(&json_records(&text)?, &[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_newlines_and_multibyte() {
        let text = "\u{feff}id,title,note\r\n1,\"ループ, 予算\",\"line one\nline two\"\r\n2,plain,\r\n\r\n";
        let (h, rows) = parse_csv(text).unwrap();
        assert_eq!(h, ["id", "title", "note"]);
        assert_eq!(
            rows,
            vec![
                vec!["1", "ループ, 予算", "line one\nline two"],
                vec!["2", "plain", ""],
            ]
        );
        let e = parse_csv("a,b\n1,2,3\n").unwrap_err().to_string();
        assert!(e.contains("line 2 has 3 fields"), "{e}");
        assert!(parse_csv("").is_err());
        assert!(parse_csv("a,b\n\"open").is_err());
        // A doubled quote inside a quoted field; a quote inside an unquoted field is literal.
        let (_, rows) = parse_csv("a,b\n\"say \"\"hi\"\"\",it's").unwrap();
        assert_eq!(rows, vec![vec!["say \"hi\"", "it's"]]);
    }

    #[test]
    fn records_become_a_typed_batch() {
        let csv = "id,score,ok,name,extra\n1,2.5,true,ループ,\n2,3,false,b,x\n";
        let (header, _) = parse_csv(csv).unwrap();
        let b = batch_from_records(&csv_records(csv).unwrap(), &header).unwrap();
        assert_eq!(b.schema.names(), ["id", "score", "ok", "name", "extra"]);
        let types: Vec<DataType> = b.schema.fields.iter().map(|f| f.data_type).collect();
        assert_eq!(
            types,
            [
                DataType::Int,
                DataType::Float,
                DataType::Bool,
                DataType::Text,
                DataType::Text
            ]
        );
        assert_eq!(b.rows[0][0], Value::Int(1));
        assert_eq!(b.rows[0][1], Value::Float(2.5));
        assert_eq!(b.rows[1][1], Value::Float(3.0));
        assert_eq!(b.rows[0][2], Value::Bool(true));
        assert_eq!(b.rows[0][3], Value::from("ループ"));
        assert_eq!(b.rows[0][4], Value::Null);
        assert_eq!(b.rows[1][4], Value::from("x"));
    }

    #[test]
    fn json_shapes() {
        let arr = json_records(r#"[{"a": 1, "b": {"c": 2}}, {"a": 2, "d": "x"}]"#).unwrap();
        let ordered = batch_from_records(&arr, &["d".to_string()]).unwrap();
        assert_eq!(ordered.schema.names(), ["d", "a", "b"]);
        let b = batch_from_records(&arr, &[]).unwrap();
        assert_eq!(b.schema.names(), ["a", "b", "d"]);
        assert_eq!(b.schema.fields[1].data_type, DataType::Json);
        assert_eq!(b.rows[1][1], Value::Null);
        assert_eq!(b.rows[1][2], Value::from("x"));
        let wrapped = json_records(r#"{"count": 1, "rows": [{"a": 1}]}"#).unwrap();
        assert_eq!(wrapped.len(), 1);
        let lines = json_records("{\"a\": 1}\n\n{\"a\": 2}\n").unwrap();
        assert_eq!(lines.len(), 2);
        let scalars = batch_from_records(&json_records("[1, 2, 3]").unwrap(), &[]).unwrap();
        assert_eq!(scalars.schema.names(), ["value"]);
        assert_eq!(scalars.rows[2][0], Value::Int(3));
        assert!(json_records("not json").is_err());
        assert!(json_records("").unwrap().is_empty());
    }

    #[test]
    fn files_by_extension() {
        let dir = tempfile::tempdir().unwrap();
        let csv = dir.path().join("t.csv");
        std::fs::write(&csv, "id,text\n1,予算\n").unwrap();
        let b = batch_from_file(&csv).unwrap();
        assert_eq!(b.rows, vec![vec![Value::Int(1), Value::from("予算")]]);
        let json = dir.path().join("t.json");
        std::fs::write(&json, "[{\"id\": 1}]").unwrap();
        assert_eq!(batch_from_file(&json).unwrap().len(), 1);
        let sjis = dir.path().join("s.csv");
        std::fs::write(&sjis, [b'a', b'\n', 0x83, 0x8B]).unwrap();
        let e = batch_from_file(&sjis).unwrap_err().to_string();
        assert!(e.contains("not UTF-8"), "{e}");
    }
}
