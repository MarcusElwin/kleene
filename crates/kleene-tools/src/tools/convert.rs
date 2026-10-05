//! Document conversion for `read` and `write_file`: the text of `.docx`,
//! `.xlsx`, `.pptx`, `.pdf` and `.eml` files, and `.docx` / `.xlsx`
//! deliverables written from Markdown or CSV.
//!
//! Office Open XML is a zip of XML parts, so the readers here walk the
//! parts with a small tag scanner and need no external program; `.pdf`
//! goes through `pdftotext` (poppler) or `pandoc`, whichever is on the
//! path. Writing a `.docx` prefers `pandoc` for fidelity (headings, lists,
//! tables) and falls back to a minimal package that Word, pandoc and
//! MarkItDown all open: one paragraph per line, headings bold, list
//! markers kept. These are the matter documents and deliverables of the
//! Harvey LAB benchmark, whose evaluator reads `.docx` through pandoc.

use crate::ToolError;
use regex::Regex;
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::sync::OnceLock;

/// Extensions `read` converts instead of returning raw bytes.
pub const DOCUMENT_EXTENSIONS: &[&str] = &["docx", "xlsx", "pptx", "pdf", "eml"];

/// Extensions `write_file` builds from text instead of writing it verbatim.
pub const DELIVERABLE_EXTENSIONS: &[&str] = &["docx", "xlsx"];

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// The text of a document at `abs`, by extension. Plain text comes back as
/// is; see the module docs for what each format turns into.
pub async fn document_text(abs: &Path) -> Result<String, ToolError> {
    let ext = ext_of(abs);
    match ext.as_str() {
        "docx" => docx_text(abs),
        "xlsx" => xlsx_text(abs),
        "pptx" => pptx_text(abs),
        "pdf" => pdf_text(abs).await,
        "eml" => Ok(eml_text(&std::fs::read(abs)?)),
        _ => Ok(lossy(std::fs::read(abs)?)),
    }
}

fn lossy(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

fn other(msg: impl Into<String>) -> ToolError {
    ToolError::Other(msg.into())
}

// ---- Office Open XML reading ---------------------------------------------

fn open_zip(abs: &Path) -> Result<zip::ZipArchive<std::fs::File>, ToolError> {
    let file = std::fs::File::open(abs)?;
    zip::ZipArchive::new(file)
        .map_err(|e| other(format!("{}: not an Office file: {e}", abs.display())))
}

fn part(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Result<Option<String>, ToolError> {
    let mut f = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(other(format!("{name}: {e}"))),
    };
    let mut s = String::new();
    f.read_to_string(&mut s)
        .map_err(|e| other(format!("{name}: {e}")))?;
    Ok(Some(s))
}

/// Decode the five XML entities and numeric character references.
pub fn unescape_xml(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else {
            out.push_str(rest);
            return out;
        };
        let ent = &rest[1..end];
        let decoded = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                u32::from_str_radix(&ent[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Escape text for an XML text node or attribute.
pub fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() && c != '\t' && c != '\n' && c != '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// A tag or a text run from an XML string, in order.
enum Tok<'a> {
    /// Element name (without namespace prefix stripped) and whether it closes
    /// (`</x>`), opens, or is empty (`<x/>`).
    Tag {
        name: &'a str,
        close: bool,
        empty: bool,
        attrs: &'a str,
    },
    Text(&'a str),
}

fn tokens(xml: &str) -> impl Iterator<Item = Tok<'_>> {
    static TAG: OnceLock<Regex> = OnceLock::new();
    let re = TAG.get_or_init(|| Regex::new(r"<(/?)([A-Za-z0-9:_.-]+)([^>]*?)(/?)>").unwrap());
    let mut pos = 0;
    let mut out = vec![];
    for m in re.captures_iter(xml) {
        let whole = m.get(0).unwrap();
        if whole.start() > pos {
            out.push(Tok::Text(&xml[pos..whole.start()]));
        }
        out.push(Tok::Tag {
            name: m.get(2).unwrap().as_str(),
            close: !m.get(1).unwrap().as_str().is_empty(),
            empty: !m.get(4).unwrap().as_str().is_empty(),
            attrs: m.get(3).unwrap().as_str(),
        });
        pos = whole.end();
    }
    if pos < xml.len() {
        out.push(Tok::Text(&xml[pos..]));
    }
    out.into_iter()
}

fn attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let i = attrs.find(&key)? + key.len();
    let rest = &attrs[i..];
    let j = rest.find('"')?;
    Some(&rest[..j])
}

/// Paragraph text of a WordprocessingML body: one line per paragraph,
/// table cells joined with ` | `, tabs and line breaks kept.
pub fn docx_body_text(xml: &str) -> String {
    let mut out = String::new();
    let mut para = String::new();
    let mut in_t = false;
    let mut in_cell = false;
    let mut row: Vec<String> = vec![];
    for t in tokens(xml) {
        match t {
            Tok::Tag {
                name, close, empty, ..
            } => match (name, close) {
                ("w:t", false) if !empty => in_t = true,
                ("w:t", true) => in_t = false,
                ("w:tab", _) if empty => para.push('\t'),
                ("w:br", _) | ("w:cr", _) if empty => para.push('\n'),
                ("w:tc", false) => {
                    in_cell = true;
                    para.clear();
                }
                ("w:tc", true) => {
                    in_cell = false;
                    row.push(para.trim().to_string());
                    para.clear();
                }
                ("w:tr", true) => {
                    out.push_str(&row.join(" | "));
                    out.push('\n');
                    row.clear();
                }
                ("w:p", true) if !in_cell => {
                    out.push_str(para.trim_end());
                    out.push('\n');
                    para.clear();
                }
                ("w:p", true) => para.push(' '),
                _ => {}
            },
            Tok::Text(s) if in_t => para.push_str(&unescape_xml(s)),
            Tok::Text(_) => {}
        }
    }
    if !para.trim().is_empty() {
        out.push_str(para.trim_end());
        out.push('\n');
    }
    out
}

fn docx_text(abs: &Path) -> Result<String, ToolError> {
    let mut zip = open_zip(abs)?;
    let body = part(&mut zip, "word/document.xml")?
        .ok_or_else(|| other(format!("{}: no word/document.xml", abs.display())))?;
    let mut text = docx_body_text(&body);
    // Margin comments, which LAB's review tasks use and its evaluator appends.
    if let Some(comments) = part(&mut zip, "word/comments.xml")? {
        let c = docx_body_text(&comments);
        if !c.trim().is_empty() {
            text.push_str("\n## Comments\n");
            text.push_str(&c);
        }
    }
    Ok(text)
}

fn pptx_text(abs: &Path) -> Result<String, ToolError> {
    let mut zip = open_zip(abs)?;
    let mut slides: Vec<(usize, String)> = zip
        .file_names()
        .filter_map(|n| {
            let num = n
                .strip_prefix("ppt/slides/slide")?
                .strip_suffix(".xml")?
                .parse::<usize>()
                .ok()?;
            Some((num, n.to_string()))
        })
        .collect();
    slides.sort();
    let mut out = String::new();
    for (num, name) in slides {
        let Some(xml) = part(&mut zip, &name)? else {
            continue;
        };
        out.push_str(&format!("## Slide {num}\n"));
        let mut para = String::new();
        let mut in_t = false;
        for t in tokens(&xml) {
            match t {
                Tok::Tag {
                    name, close, empty, ..
                } => match (name, close) {
                    ("a:t", false) if !empty => in_t = true,
                    ("a:t", true) => in_t = false,
                    ("a:br", _) => para.push('\n'),
                    ("a:p", true) => {
                        if !para.trim().is_empty() {
                            out.push_str(para.trim_end());
                            out.push('\n');
                        }
                        para.clear();
                    }
                    _ => {}
                },
                Tok::Text(s) if in_t => para.push_str(&unescape_xml(s)),
                Tok::Text(_) => {}
            }
        }
        out.push('\n');
    }
    Ok(out)
}

fn column_index(cell_ref: &str) -> usize {
    let mut n = 0usize;
    for c in cell_ref.chars().take_while(|c| c.is_ascii_alphabetic()) {
        n = n * 26 + (c.to_ascii_uppercase() as usize - 'A' as usize + 1);
    }
    n.saturating_sub(1)
}

fn csv_cell(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// The rows of a SpreadsheetML worksheet as CSV lines, given the shared
/// string table.
pub fn xlsx_sheet_csv(xml: &str, shared: &[String]) -> String {
    let mut out = String::new();
    let mut row: Vec<String> = vec![];
    let mut cell_col = 0usize;
    let mut cell_type = String::new();
    let mut in_v = false;
    let mut in_is_t = false;
    let mut value = String::new();
    let mut in_cell = false;
    let mut in_row = false;
    for t in tokens(xml) {
        match t {
            Tok::Tag {
                name,
                close,
                empty,
                attrs,
            } => match (name, close) {
                ("row", false) if !empty => {
                    in_row = true;
                    row.clear();
                }
                ("row", true) | ("row", false) => {
                    if in_row || empty {
                        out.push_str(
                            &row.iter()
                                .map(|c| csv_cell(c))
                                .collect::<Vec<_>>()
                                .join(","),
                        );
                        out.push('\n');
                    }
                    in_row = false;
                    row.clear();
                }
                ("c", false) => {
                    cell_col = attr(attrs, "r").map(column_index).unwrap_or(row.len());
                    cell_type = attr(attrs, "t").unwrap_or("").to_string();
                    value.clear();
                    in_cell = !empty;
                }
                ("c", true) => {
                    let text = if cell_type == "s" {
                        value
                            .trim()
                            .parse::<usize>()
                            .ok()
                            .and_then(|i| shared.get(i).cloned())
                            .unwrap_or_default()
                    } else if cell_type == "b" {
                        if value.trim() == "1" { "TRUE" } else { "FALSE" }.to_string()
                    } else {
                        value.clone()
                    };
                    while row.len() < cell_col {
                        row.push(String::new());
                    }
                    if row.len() == cell_col {
                        row.push(text);
                    } else {
                        row[cell_col] = text;
                    }
                    in_cell = false;
                }
                ("v", false) if in_cell && !empty => in_v = true,
                ("v", true) => in_v = false,
                ("t", false) if in_cell && !empty => in_is_t = true,
                ("t", true) => in_is_t = false,
                _ => {}
            },
            Tok::Text(s) if in_v || in_is_t => value.push_str(&unescape_xml(s)),
            Tok::Text(_) => {}
        }
    }
    out
}

fn xlsx_text(abs: &Path) -> Result<String, ToolError> {
    let mut zip = open_zip(abs)?;
    // Shared strings: one entry per <si>, its <t> runs concatenated.
    let mut shared: Vec<String> = vec![];
    if let Some(xml) = part(&mut zip, "xl/sharedStrings.xml")? {
        let mut cur = String::new();
        let mut in_t = false;
        for t in tokens(&xml) {
            match t {
                Tok::Tag {
                    name, close, empty, ..
                } => match (name, close) {
                    ("si", false) => cur.clear(),
                    ("si", true) => shared.push(std::mem::take(&mut cur)),
                    ("t", false) if !empty => in_t = true,
                    ("t", true) => in_t = false,
                    _ => {}
                },
                Tok::Text(s) if in_t => cur.push_str(&unescape_xml(s)),
                Tok::Text(_) => {}
            }
        }
    }
    // Sheet names in workbook order, resolved to parts through the rels.
    let workbook = part(&mut zip, "xl/workbook.xml")?.unwrap_or_default();
    let rels = part(&mut zip, "xl/_rels/workbook.xml.rels")?.unwrap_or_default();
    let mut targets: Vec<(String, String)> = vec![];
    for t in tokens(&rels) {
        if let Tok::Tag {
            name: "Relationship",
            attrs,
            ..
        } = t
        {
            if let (Some(id), Some(target)) = (attr(attrs, "Id"), attr(attrs, "Target")) {
                targets.push((id.to_string(), target.trim_start_matches('/').to_string()));
            }
        }
    }
    let mut sheets: Vec<(String, String)> = vec![];
    for t in tokens(&workbook) {
        if let Tok::Tag {
            name: "sheet",
            attrs,
            ..
        } = t
        {
            let name = attr(attrs, "name").map(unescape_xml).unwrap_or_default();
            let rid = attr(attrs, "r:id").unwrap_or("");
            let target = targets.iter().find(|(id, _)| id == rid).map(|(_, t)| {
                if t.starts_with("xl/") {
                    t.clone()
                } else {
                    format!("xl/{t}")
                }
            });
            if let Some(target) = target {
                sheets.push((name, target));
            }
        }
    }
    if sheets.is_empty() {
        let mut names: Vec<String> = zip
            .file_names()
            .filter(|n| n.starts_with("xl/worksheets/sheet") && n.ends_with(".xml"))
            .map(str::to_string)
            .collect();
        names.sort();
        sheets = names.iter().map(|n| (n.clone(), n.clone())).collect();
    }
    let mut out = String::new();
    for (name, target) in sheets {
        let Some(xml) = part(&mut zip, &target)? else {
            continue;
        };
        out.push_str(&format!("## Sheet: {name}\n"));
        out.push_str(&xlsx_sheet_csv(&xml, &shared));
        out.push('\n');
    }
    Ok(out)
}

// ---- PDF and email ---------------------------------------------------------

async fn pdf_text(abs: &Path) -> Result<String, ToolError> {
    let attempts: [(&str, Vec<String>); 2] = [
        (
            "pdftotext",
            vec!["-layout".into(), abs.display().to_string(), "-".into()],
        ),
        (
            "pandoc",
            vec![abs.display().to_string(), "-t".into(), "plain".into()],
        ),
    ];
    let mut missing = vec![];
    for (bin, args) in attempts {
        match tokio::process::Command::new(bin).args(&args).output().await {
            Ok(out) if out.status.success() => return Ok(lossy(out.stdout)),
            Ok(out) => {
                return Err(other(format!(
                    "{bin} failed on {}: {}",
                    abs.display(),
                    String::from_utf8_lossy(&out.stderr).trim()
                )))
            }
            Err(_) => missing.push(bin),
        }
    }
    Err(other(format!(
        "no PDF converter on the path ({}); install poppler-utils or pandoc",
        missing.join(", ")
    )))
}

fn decode_quoted_printable(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                i += 2;
                continue;
            }
            if i + 2 < bytes.len() && bytes[i + 1] == b'\r' && bytes[i + 2] == b'\n' {
                i += 3;
                continue;
            }
            if i + 2 < bytes.len() {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    lossy(out)
}

/// An email: headers kept, a quoted-printable body decoded (applied again
/// to a nested message, the shape a forwarded or exported mail has).
pub fn eml_text(bytes: &[u8]) -> String {
    let mut text = lossy(bytes.to_vec()).replace("\r\n", "\n");
    for _ in 0..2 {
        let lower_head: String = text.split("\n\n").next().unwrap_or("").to_ascii_lowercase();
        if lower_head.contains("content-transfer-encoding: quoted-printable") {
            text = decode_quoted_printable(&text);
        } else {
            break;
        }
    }
    text
}

// ---- Writing deliverables --------------------------------------------------

fn pandoc_available() -> bool {
    std::process::Command::new("pandoc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Write `text` as the document `abs` should hold: Markdown becomes a
/// `.docx` (through `pandoc` when installed, else a minimal package), CSV
/// becomes a one-sheet `.xlsx`. Returns the bytes written.
pub async fn write_document(abs: &Path, text: &str) -> Result<u64, ToolError> {
    match ext_of(abs).as_str() {
        "docx" => {
            if pandoc_available() {
                let mut child = tokio::process::Command::new("pandoc")
                    .args(["-f", "markdown", "-t", "docx", "-o"])
                    .arg(abs)
                    .stdin(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()?;
                if let Some(mut stdin) = child.stdin.take() {
                    use tokio::io::AsyncWriteExt;
                    stdin.write_all(text.as_bytes()).await?;
                }
                let out = child.wait_with_output().await?;
                if !out.status.success() {
                    return Err(other(format!(
                        "pandoc failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )));
                }
            } else {
                write_minimal_docx(abs, text)?;
            }
        }
        "xlsx" => write_minimal_xlsx(abs, text)?,
        _ => std::fs::write(abs, text)?,
    }
    Ok(std::fs::metadata(abs)?.len())
}

fn zip_writer(abs: &Path) -> Result<zip::ZipWriter<std::fs::File>, ToolError> {
    Ok(zip::ZipWriter::new(std::fs::File::create(abs)?))
}

fn add_part(
    z: &mut zip::ZipWriter<std::fs::File>,
    name: &str,
    body: &str,
) -> Result<(), ToolError> {
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    z.start_file(name, opts)
        .map_err(|e| other(format!("{name}: {e}")))?;
    z.write_all(body.as_bytes())?;
    Ok(())
}

/// A paragraph of the minimal `.docx`: Markdown headings become bold
/// paragraphs with a `Heading` style, list markers are kept as text, table
/// rows keep their pipes, everything else is one paragraph per line.
fn docx_paragraph(line: &str) -> String {
    let trimmed = line.trim_end();
    let (style, bold, text) = if let Some(rest) = trimmed.strip_prefix("### ") {
        (Some("Heading3"), true, rest)
    } else if let Some(rest) = trimmed.strip_prefix("## ") {
        (Some("Heading2"), true, rest)
    } else if let Some(rest) = trimmed.strip_prefix("# ") {
        (Some("Heading1"), true, rest)
    } else {
        (None, false, trimmed)
    };
    // Inline emphasis markers would read as literal asterisks; drop the
    // common ones.
    let text = text.replace("**", "");
    let ppr = style
        .map(|s| format!("<w:pPr><w:pStyle w:val=\"{s}\"/></w:pPr>"))
        .unwrap_or_default();
    let rpr = if bold { "<w:rPr><w:b/></w:rPr>" } else { "" };
    format!(
        "<w:p>{ppr}<w:r>{rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
        escape_xml(&text)
    )
}

fn write_minimal_docx(abs: &Path, text: &str) -> Result<(), ToolError> {
    let separator = |l: &str| {
        let t = l.trim();
        t.starts_with('|') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
    };
    let body: String = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !separator(l))
        .map(docx_paragraph)
        .collect();
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}<w:sectPr/></w:body></w:document>"
    );
    let mut z = zip_writer(abs)?;
    add_part(&mut z, "[Content_Types].xml", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
</Types>")?;
    add_part(&mut z, "_rels/.rels", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
</Relationships>")?;
    add_part(&mut z, "word/_rels/document.xml.rels", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
</Relationships>")?;
    add_part(&mut z, "word/styles.xml", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/><w:rPr><w:b/><w:sz w:val=\"32\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/><w:basedOn w:val=\"Normal\"/><w:rPr><w:b/><w:sz w:val=\"28\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading3\"><w:name w:val=\"heading 3\"/><w:basedOn w:val=\"Normal\"/><w:rPr><w:b/><w:sz w:val=\"24\"/></w:rPr></w:style>\
</w:styles>")?;
    add_part(&mut z, "word/document.xml", &document)?;
    z.finish().map_err(|e| other(e.to_string()))?;
    Ok(())
}

/// Split one CSV line into cells (double-quote quoting, `""` escapes).
pub fn csv_fields(line: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn column_name(mut i: usize) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (i % 26) as u8) as char);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    s
}

fn write_minimal_xlsx(abs: &Path, text: &str) -> Result<(), ToolError> {
    let mut rows = String::new();
    for (r, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let mut cells = String::new();
        for (c, field) in csv_fields(line).iter().enumerate() {
            let r#ref = format!("{}{}", column_name(c), r + 1);
            let numeric = field.trim().parse::<f64>().is_ok() && !field.trim().is_empty();
            if numeric {
                cells.push_str(&format!("<c r=\"{ref}\"><v>{}</v></c>", field.trim()));
            } else {
                cells.push_str(&format!(
                    "<c r=\"{ref}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                    escape_xml(field)
                ));
            }
        }
        rows.push_str(&format!("<row r=\"{}\">{cells}</row>", r + 1));
    }
    let sheet = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{rows}</sheetData></worksheet>"
    );
    let mut z = zip_writer(abs)?;
    add_part(&mut z, "[Content_Types].xml", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
<Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
<Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\
</Types>")?;
    add_part(&mut z, "_rels/.rels", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
</Relationships>")?;
    add_part(&mut z, "xl/workbook.xml", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
<sheets><sheet name=\"Sheet1\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>")?;
    add_part(&mut z, "xl/_rels/workbook.xml.rels", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
</Relationships>")?;
    add_part(&mut z, "xl/styles.xml", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
<fonts count=\"1\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts>\
<fills count=\"1\"><fill><patternFill patternType=\"none\"/></fill></fills>\
<borders count=\"1\"><border/></borders>\
<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
<cellXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/></cellXfs>\
</styleSheet>")?;
    add_part(&mut z, "xl/worksheets/sheet1.xml", &sheet)?;
    z.finish().map_err(|e| other(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_entities_round_trip() {
        assert_eq!(
            unescape_xml("P&amp;L &lt;b&gt; &#x27;q&#39;"),
            "P&L <b> 'q'"
        );
        assert_eq!(escape_xml("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
        assert_eq!(unescape_xml(&escape_xml("x & y < z")), "x & y < z");
    }

    #[test]
    fn docx_body_reads_paragraphs_tabs_and_tables() {
        let xml = r#"<w:document><w:body><w:p><w:r><w:t>Title</w:t></w:r></w:p><w:p><w:r><w:t xml:space="preserve">A </w:t></w:r><w:r><w:tab/><w:t>B &amp; C</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>h1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>h2</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>2</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:t>End</w:t></w:r></w:p></w:body></w:document>"#;
        assert_eq!(
            docx_body_text(xml),
            "Title\nA \tB & C\nh1 | h2\n1 | 2\nEnd\n"
        );
    }

    #[test]
    fn xlsx_sheet_reads_shared_inline_and_numbers_into_csv() {
        let shared = vec!["Revenue".to_string(), "Say \"hi\", twice".to_string()];
        let xml = r#"<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="C1"><v>12.5</v></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>Cost</t></is></c><c r="B2" t="s"><v>1</v></c><c r="C2" t="b"><v>1</v></c></row></sheetData></worksheet>"#;
        assert_eq!(
            xlsx_sheet_csv(xml, &shared),
            "Revenue,,12.5\nCost,\"Say \"\"hi\"\", twice\",TRUE\n"
        );
        assert_eq!(csv_fields("a,\"b,c\",\"d\"\"e\""), vec!["a", "b,c", "d\"e"]);
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(27), "AB");
        assert_eq!(column_index("AB7"), 27);
    }

    #[test]
    fn quoted_printable_email_is_decoded_twice_when_nested() {
        let raw = "Content-Type: text/plain\nContent-Transfer-Encoding: quoted-printable\n\nSubject: FYI =E2=80=94 Flag\nLong line that is soft =\nwrapped.\n";
        let text = eml_text(raw.as_bytes());
        assert!(text.contains("FYI — Flag"), "{text}");
        assert!(text.contains("soft wrapped."), "{text}");
    }

    #[tokio::test]
    async fn written_docx_and_xlsx_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let docx = dir.path().join("memo.docx");
        write_minimal_docx(
            &docx,
            "# Antitrust Memo\n\nThe **deal** raises HSR questions.\n- Item one\n| a | b |\n|---|---|\n| 1 | 2 |\n",
        )
        .unwrap();
        let text = document_text(&docx).await.unwrap();
        assert_eq!(
            text,
            "Antitrust Memo\nThe deal raises HSR questions.\n- Item one\n| a | b |\n| 1 | 2 |\n"
        );
        let xlsx = dir.path().join("model.xlsx");
        write_minimal_xlsx(&xlsx, "Item,FY2023\nRevenue,\"1,200\"\nMargin,0.42\n").unwrap();
        let text = document_text(&xlsx).await.unwrap();
        assert_eq!(
            text,
            "## Sheet: Sheet1\nItem,FY2023\nRevenue,\"1,200\"\nMargin,0.42\n\n"
        );
        let n = write_document(&dir.path().join("plain.md"), "hello")
            .await
            .unwrap();
        assert_eq!(n, 5);
    }

    #[tokio::test]
    async fn not_a_zip_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.docx");
        std::fs::write(&p, "not a zip").unwrap();
        let err = document_text(&p).await.unwrap_err().to_string();
        assert!(err.contains("not an Office file"), "{err}");
    }
}
