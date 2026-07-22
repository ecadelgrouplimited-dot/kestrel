//! Writing real office documents.
//!
//! The agent thinks in Markdown, but people need `.docx` and `.xlsx`. Both are
//! ZIP archives of XML, so we build the parts here and zip them with
//! PowerShell's built-in compression — **no Office install, no extra crates**,
//! the same approach [`crate::office`] uses for reading.
//!
//! The Markdown subset covered is the one reports actually use: headings,
//! paragraphs, bullet and numbered lists, block quotes, tables, horizontal
//! rules, and inline `**bold**` / `*italic*` / `` `code` ``.

use std::path::Path;
use std::process::Command;

/// Write `markdown` to `path` as a real Word document.
pub fn write_docx(path: &Path, markdown: &str) -> Result<(), String> {
    let parts = vec![
        ("[Content_Types].xml", CONTENT_TYPES.to_string()),
        ("_rels/.rels", ROOT_RELS.to_string()),
        ("word/_rels/document.xml.rels", DOC_RELS.to_string()),
        ("word/styles.xml", STYLES.to_string()),
        ("word/settings.xml", SETTINGS.to_string()),
        ("word/document.xml", markdown_to_document_xml(markdown)),
    ];
    zip_parts(path, &parts)
}

/// Write delimited `data` (CSV or TSV) to `path` as a real Excel workbook.
pub fn write_xlsx(path: &Path, data: &str, sheet_name: &str) -> Result<(), String> {
    let rows = parse_delimited(data);
    if rows.is_empty() {
        return Err("no rows to write".to_string());
    }
    let name = if sheet_name.trim().is_empty() {
        "Sheet1"
    } else {
        sheet_name.trim()
    };
    let parts = vec![
        ("[Content_Types].xml", XLSX_CONTENT_TYPES.to_string()),
        ("_rels/.rels", XLSX_ROOT_RELS.to_string()),
        ("xl/_rels/workbook.xml.rels", XLSX_WB_RELS.to_string()),
        ("xl/styles.xml", XLSX_STYLES.to_string()),
        ("xl/workbook.xml", workbook_xml(name)),
        ("xl/worksheets/sheet1.xml", sheet_xml(&rows)),
    ];
    zip_parts(path, &parts)
}

/// Split CSV/TSV text into rows of cells. Tabs win when present on a line;
/// otherwise commas, honouring simple double-quoted fields.
fn parse_delimited(data: &str) -> Vec<Vec<String>> {
    data.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            if line.contains('\t') {
                return line.split('\t').map(|c| c.trim().to_string()).collect();
            }
            let mut cells = Vec::new();
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
                    ',' if !quoted => cells.push(std::mem::take(&mut cur).trim().to_string()),
                    _ => cur.push(c),
                }
            }
            cells.push(cur.trim().to_string());
            cells
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Word
// ---------------------------------------------------------------------------

/// Convert a Markdown document into the body of `word/document.xml`.
pub fn markdown_to_document_xml(markdown: &str) -> String {
    let mut body = String::new();
    let lines: Vec<&str> = markdown.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // Table: a run of lines starting with '|'.
        if trimmed.starts_with('|') {
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].trim().starts_with('|') {
                let cells: Vec<String> = lines[i]
                    .trim()
                    .trim_matches('|')
                    .split('|')
                    .map(|c| c.trim().to_string())
                    .collect();
                // Skip the |---|---| separator row.
                if !cells.iter().all(|c| {
                    !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')
                }) {
                    rows.push(cells);
                }
                i += 1;
            }
            if !rows.is_empty() {
                body.push_str(&table_xml(&rows));
            }
            continue;
        }

        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        // Horizontal rule → an empty spacer paragraph.
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            body.push_str("<w:p/>");
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("### ") {
            body.push_str(&para_xml(rest, Some("Heading3"), false));
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            body.push_str(&para_xml(rest, Some("Heading2"), false));
        } else if let Some(rest) = trimmed.strip_prefix("# ") {
            body.push_str(&para_xml(rest, Some("Heading1"), false));
        } else if let Some(rest) = trimmed.strip_prefix("> ") {
            body.push_str(&para_xml(rest, Some("Quote"), false));
        } else if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            body.push_str(&para_xml(&format!("• {rest}"), None, true));
        } else if let Some((num, rest)) = numbered_item(trimmed) {
            body.push_str(&para_xml(&format!("{num}. {rest}"), None, true));
        } else {
            body.push_str(&para_xml(trimmed, None, false));
        }
        i += 1;
    }
    format!("{DOC_OPEN}{body}{DOC_CLOSE}")
}

/// Recognise `1. text` style list items.
fn numbered_item(line: &str) -> Option<(String, String)> {
    let (num, rest) = line.split_once(". ")?;
    if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
        Some((num.to_string(), rest.to_string()))
    } else {
        None
    }
}

/// One paragraph, optionally styled and/or indented (for list items).
fn para_xml(text: &str, style: Option<&str>, indent: bool) -> String {
    let mut props = String::new();
    if let Some(style) = style {
        props.push_str(&format!("<w:pStyle w:val=\"{style}\"/>"));
    }
    if indent {
        props.push_str("<w:ind w:left=\"360\"/>");
    }
    let props = if props.is_empty() {
        String::new()
    } else {
        format!("<w:pPr>{props}</w:pPr>")
    };
    format!("<w:p>{props}{}</w:p>", runs_xml(text))
}

/// Split inline Markdown into styled runs (`**bold**`, `*italic*`, `` `code` ``).
fn runs_xml(text: &str) -> String {
    let mut out = String::new();
    let mut buf = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    // Flush pending plain text as a run.
    let flush = |buf: &mut String, out: &mut String| {
        if !buf.is_empty() {
            out.push_str(&run_xml(buf, ""));
            buf.clear();
        }
    };
    while i < chars.len() {
        let rest: String = chars[i..].iter().collect();
        if let Some(inner) = delimited(&rest, "**") {
            flush(&mut buf, &mut out);
            out.push_str(&run_xml(&inner, "<w:b/>"));
            i += inner.chars().count() + 4;
            continue;
        }
        if let Some(inner) = delimited(&rest, "`") {
            flush(&mut buf, &mut out);
            out.push_str(&run_xml(
                &inner,
                "<w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\"/>",
            ));
            i += inner.chars().count() + 2;
            continue;
        }
        if let Some(inner) = delimited(&rest, "*") {
            flush(&mut buf, &mut out);
            out.push_str(&run_xml(&inner, "<w:i/>"));
            i += inner.chars().count() + 2;
            continue;
        }
        buf.push(chars[i]);
        i += 1;
    }
    flush(&mut buf, &mut out);
    if out.is_empty() {
        out.push_str(&run_xml("", ""));
    }
    out
}

/// If `s` opens with `delim`, return the text up to the closing `delim`.
fn delimited(s: &str, delim: &str) -> Option<String> {
    let rest = s.strip_prefix(delim)?;
    let end = rest.find(delim)?;
    if end == 0 {
        return None;
    }
    Some(rest[..end].to_string())
}

/// A single run with optional run-properties.
fn run_xml(text: &str, props: &str) -> String {
    let props = if props.is_empty() {
        String::new()
    } else {
        format!("<w:rPr>{props}</w:rPr>")
    };
    format!(
        "<w:r>{props}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
        xml_escape(text)
    )
}

/// A bordered table; the first row is emboldened as a header.
fn table_xml(rows: &[Vec<String>]) -> String {
    let mut out = String::from(
        "<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>\
         <w:top w:val=\"single\" w:sz=\"4\" w:color=\"999999\"/>\
         <w:left w:val=\"single\" w:sz=\"4\" w:color=\"999999\"/>\
         <w:bottom w:val=\"single\" w:sz=\"4\" w:color=\"999999\"/>\
         <w:right w:val=\"single\" w:sz=\"4\" w:color=\"999999\"/>\
         <w:insideH w:val=\"single\" w:sz=\"4\" w:color=\"999999\"/>\
         <w:insideV w:val=\"single\" w:sz=\"4\" w:color=\"999999\"/>\
         </w:tblBorders></w:tblPr>",
    );
    for (r, row) in rows.iter().enumerate() {
        out.push_str("<w:tr>");
        for cell in row {
            let content = if r == 0 {
                format!("<w:p>{}</w:p>", run_xml(cell, "<w:b/>"))
            } else {
                format!("<w:p>{}</w:p>", runs_xml(cell))
            };
            out.push_str(&format!("<w:tc><w:tcPr/>{content}</w:tc>"));
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl>");
    out
}

// ---------------------------------------------------------------------------
// Excel
// ---------------------------------------------------------------------------

/// The A1-style column name for a zero-based index (0 → A, 26 → AA).
pub fn column_name(mut index: usize) -> String {
    let mut name = String::new();
    loop {
        name.insert(0, (b'A' + (index % 26) as u8) as char);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    name
}

/// Build `xl/worksheets/sheet1.xml`: a frozen bold header, columns sized to the
/// content, numbers stored as numbers, and `=` cells written as live formulas.
fn sheet_xml(rows: &[Vec<String>]) -> String {
    let mut body = String::new();
    for (r, row) in rows.iter().enumerate() {
        body.push_str(&format!("<row r=\"{}\">", r + 1));
        for (c, cell) in row.iter().enumerate() {
            let reference = format!("{}{}", column_name(c), r + 1);
            // Row 1 uses the bold style so the header stands out.
            let style = if r == 0 { " s=\"1\"" } else { "" };
            if let Some(formula) = cell.strip_prefix('=') {
                // A live formula — Excel evaluates it on open.
                body.push_str(&format!(
                    "<c r=\"{reference}\"{style}><f>{}</f></c>",
                    xml_escape(formula)
                ));
            } else if !cell.is_empty() && cell.parse::<f64>().is_ok() {
                // Numeric, so Excel can sum, sort, and chart it.
                body.push_str(&format!("<c r=\"{reference}\"{style}><v>{cell}</v></c>"));
            } else {
                body.push_str(&format!(
                    "<c r=\"{reference}\"{style} t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                    xml_escape(cell)
                ));
            }
        }
        body.push_str("</row>");
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
         <sheetViews><sheetView workbookViewId=\"0\">\
         <pane ySplit=\"1\" topLeftCell=\"A2\" activePane=\"bottomLeft\" state=\"frozen\"/>\
         </sheetView></sheetViews>\
         {cols}<sheetData>{body}</sheetData></worksheet>",
        cols = cols_xml(rows)
    )
}

/// Size each column to its widest cell, clamped to something readable.
fn cols_xml(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if columns == 0 {
        return String::new();
    }
    let mut out = String::from("<cols>");
    for c in 0..columns {
        let widest = rows
            .iter()
            .filter_map(|r| r.get(c))
            .map(|cell| cell.chars().count())
            .max()
            .unwrap_or(8);
        let width = (widest + 2).clamp(9, 60);
        out.push_str(&format!(
            "<col min=\"{n}\" max=\"{n}\" width=\"{width}\" customWidth=\"1\"/>",
            n = c + 1
        ));
    }
    out.push_str("</cols>");
    out
}

fn workbook_xml(sheet_name: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <sheets><sheet name=\"{}\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>",
        xml_escape(sheet_name)
    )
}

// ---------------------------------------------------------------------------
// Packaging
// ---------------------------------------------------------------------------

/// Escape text for XML content.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Write the parts into a staging directory and zip it to `path`.
fn zip_parts(path: &Path, parts: &[(&str, String)]) -> Result<(), String> {
    let stage = std::env::temp_dir().join(format!(
        "kestrel-docwrite-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let cleanup = |dir: &Path| {
        let _ = std::fs::remove_dir_all(dir);
    };
    for (name, contents) in parts {
        let full = stage.join(name);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                cleanup(&stage);
                e.to_string()
            })?;
        }
        std::fs::write(&full, contents).map_err(|e| {
            cleanup(&stage);
            e.to_string()
        })?;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Overwrite cleanly: creating a zip refuses an existing target.
    let _ = std::fs::remove_file(path);
    // Entries are added one by one with explicit forward-slash names. .NET
    // Framework's CreateFromDirectory writes `word\document.xml`, which violates
    // the ZIP spec — Word and Excel then reject the file as corrupt.
    let script = format!(
        "$ErrorActionPreference = 'Stop'\n\
         Add-Type -AssemblyName System.IO.Compression\n\
         Add-Type -AssemblyName System.IO.Compression.FileSystem\n\
         $stage = {src}\n\
         $zip = [IO.Compression.ZipFile]::Open({dst}, [IO.Compression.ZipArchiveMode]::Create)\n\
         try {{\n\
         \x20 Get-ChildItem -LiteralPath $stage -Recurse -File | ForEach-Object {{\n\
         \x20   $rel = $_.FullName.Substring($stage.Length + 1).Replace('\\', '/')\n\
         \x20   [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $_.FullName, $rel)\n\
         \x20 }}\n\
         }} finally {{ $zip.Dispose() }}\n",
        src = ps_quote(&stage.display().to_string()),
        dst = ps_quote(&path.display().to_string()),
    );
    let result = run_powershell(&script);
    cleanup(&stage);
    result.map(|_| ())
}

fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Run a PowerShell script from a temp file, draining output, with a timeout.
fn run_powershell(script: &str) -> Result<String, String> {
    use std::io::Read;
    let file = std::env::temp_dir().join(format!(
        "kestrel-docwrite-{}-{}.ps1",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&file, script).map_err(|e| e.to_string())?;
    let mut child = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
        ])
        .arg("-File")
        .arg(&file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run PowerShell: {e}"))?;

    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_string(&mut buf);
        }
        buf
    });
    let err_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_string(&mut buf);
        }
        buf
    });

    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if started.elapsed() > std::time::Duration::from_secs(45) {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&file);
                    return Err("writing the document timed out".to_string());
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&file);
                return Err(format!("could not run PowerShell: {e}"));
            }
        }
    };
    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();
    let _ = std::fs::remove_file(&file);
    if status.success() {
        Ok(stdout)
    } else {
        Err(format!("could not write the document: {}", stderr.trim()))
    }
}

// ---------------------------------------------------------------------------
// Static package parts
// ---------------------------------------------------------------------------

const DOC_OPEN: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>";
const DOC_CLOSE: &str = "</w:body></w:document>";

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
<Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/>\
</Types>";

const ROOT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
</Relationships>";

const DOC_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings\" Target=\"settings.xml\"/>\
</Relationships>";

/// Declaring the modern compatibility mode stops Word opening our documents in
/// "Compatibility Mode".
const SETTINGS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:compat><w:compatSetting w:name=\"compatibilityMode\" \
w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"15\"/></w:compat>\
</w:settings>";

/// Minimal styles so headings and quotes render like a normal Word document.
const STYLES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/>\
<w:pPr><w:outlineLvl w:val=\"0\"/><w:spacing w:before=\"280\" w:after=\"120\"/></w:pPr>\
<w:rPr><w:b/><w:sz w:val=\"36\"/><w:color w:val=\"1F3864\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/>\
<w:pPr><w:outlineLvl w:val=\"1\"/><w:spacing w:before=\"240\" w:after=\"100\"/></w:pPr>\
<w:rPr><w:b/><w:sz w:val=\"28\"/><w:color w:val=\"2E5496\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading3\"><w:name w:val=\"heading 3\"/>\
<w:pPr><w:outlineLvl w:val=\"2\"/><w:spacing w:before=\"200\" w:after=\"80\"/></w:pPr>\
<w:rPr><w:b/><w:sz w:val=\"24\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Quote\"><w:name w:val=\"Quote\"/>\
<w:pPr><w:ind w:left=\"420\"/></w:pPr><w:rPr><w:i/><w:color w:val=\"555555\"/></w:rPr></w:style>\
</w:styles>";

const XLSX_CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
<Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
<Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\
</Types>";

/// Two cell formats: 0 = normal, 1 = bold (used for the header row). Excel
/// requires the first two fills to be `none` and `gray125`.
const XLSX_STYLES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
<fonts count=\"2\">\
<font><sz val=\"11\"/><name val=\"Calibri\"/></font>\
<font><b/><sz val=\"11\"/><name val=\"Calibri\"/></font>\
</fonts>\
<fills count=\"2\">\
<fill><patternFill patternType=\"none\"/></fill>\
<fill><patternFill patternType=\"gray125\"/></fill>\
</fills>\
<borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders>\
<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
<cellXfs count=\"2\">\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
<xf numFmtId=\"0\" fontId=\"1\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyFont=\"1\"/>\
</cellXfs></styleSheet>";

const XLSX_ROOT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
</Relationships>";

const XLSX_WB_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
</Relationships>";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_paragraphs_and_lists_map_to_styles() {
        let xml = markdown_to_document_xml(
            "# Title\n\nIntro text.\n\n## Section\n\n- first\n- second\n\n> a quote\n",
        );
        assert!(xml.contains("w:val=\"Heading1\""));
        assert!(xml.contains("w:val=\"Heading2\""));
        assert!(xml.contains("w:val=\"Quote\""));
        assert!(xml.contains("• first"));
        assert!(xml.contains("Intro text."));
        assert!(xml.starts_with("<?xml"));
        assert!(xml.ends_with("</w:document>"));
    }

    #[test]
    fn inline_bold_italic_and_code_become_runs() {
        let xml = markdown_to_document_xml("Plain **bold** and *italic* and `code` here.");
        assert!(xml.contains("<w:b/>"), "bold run missing");
        assert!(xml.contains("<w:i/>"), "italic run missing");
        assert!(xml.contains("Consolas"), "code run missing");
        assert!(xml.contains("Plain "));
    }

    #[test]
    fn tables_become_word_tables_with_a_bold_header() {
        let xml = markdown_to_document_xml("| Name | Value |\n|---|---|\n| Kampala | 42 |\n");
        assert!(xml.contains("<w:tbl>"));
        // The separator row must not become a table row: header + 1 data row.
        assert_eq!(xml.matches("<w:tr>").count(), 2);
        assert!(xml.contains("Kampala"));
    }

    #[test]
    fn xml_special_characters_are_escaped() {
        let xml = markdown_to_document_xml("Tom & Jerry <tag> \"quoted\"");
        assert!(xml.contains("Tom &amp; Jerry"));
        assert!(xml.contains("&lt;tag&gt;"));
        assert!(!xml.contains("<tag>"));
    }

    #[test]
    fn spreadsheet_columns_and_types() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(column_name(27), "AB");

        let rows = parse_delimited("Region,Revenue\nKampala,1500\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], vec!["Kampala", "1500"]);

        let xml = sheet_xml(&rows);
        // Text is inline; numbers are stored as numbers so Excel can sum them.
        assert!(xml.contains("t=\"inlineStr\""));
        assert!(xml.contains("<c r=\"B2\"><v>1500</v></c>"));
    }

    #[test]
    fn formulas_header_and_layout() {
        let rows =
            parse_delimited("Region,Revenue\nKampala,1500\nNairobi,900\nTotal,=SUM(B2:B3)\n");
        let xml = sheet_xml(&rows);
        // `=` becomes a live formula Excel evaluates, not text.
        assert!(xml.contains("<f>SUM(B2:B3)</f>"), "formula missing: {xml}");
        assert!(!xml.contains("=SUM"), "formula must not be stored as text");
        // The header row is styled and frozen, and columns are sized.
        assert!(xml.contains("<c r=\"A1\" s=\"1\""), "header not bold");
        assert!(
            !xml.contains("<c r=\"A2\" s=\"1\""),
            "only row 1 is a header"
        );
        assert!(xml.contains("state=\"frozen\""), "header not frozen");
        assert!(xml.contains("<cols>"), "column widths missing");
    }

    #[test]
    fn column_widths_track_the_widest_cell() {
        let rows = vec![
            vec!["ab".to_string(), "a-very-long-heading-value".to_string()],
            vec!["c".to_string(), "d".to_string()],
        ];
        let cols = cols_xml(&rows);
        // Narrow column clamps to the readable minimum; wide one grows.
        assert!(cols.contains("min=\"1\" max=\"1\" width=\"9\""));
        assert!(cols.contains("min=\"2\" max=\"2\" width=\"27\""));
    }

    #[test]
    fn quoted_csv_fields_survive_commas() {
        let rows = parse_delimited("name,note\n\"Doe, Jane\",\"said \"\"hi\"\"\"\n");
        assert_eq!(rows[1][0], "Doe, Jane");
        assert_eq!(rows[1][1], "said \"hi\"");
    }

    #[test]
    fn tabs_take_precedence_over_commas() {
        let rows = parse_delimited("a,b\tc,d\n");
        assert_eq!(rows[0], vec!["a,b", "c,d"]);
    }
}
