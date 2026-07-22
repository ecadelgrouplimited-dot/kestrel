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

/// One sheet of a workbook: a tab name and its CSV/TSV rows.
#[derive(Debug, Clone)]
pub struct Sheet {
    pub name: String,
    pub data: String,
}

/// The shape of an embedded chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartKind {
    Bar,
    Line,
    Pie,
}

impl ChartKind {
    /// Parse a user/model-supplied chart type.
    pub fn parse(kind: &str) -> Option<Self> {
        match kind.trim().to_ascii_lowercase().as_str() {
            "bar" | "column" | "bars" => Some(ChartKind::Bar),
            "line" | "lines" | "trend" => Some(ChartKind::Line),
            "pie" | "donut" | "doughnut" => Some(ChartKind::Pie),
            _ => None,
        }
    }
    fn element(self) -> &'static str {
        match self {
            ChartKind::Bar => "barChart",
            ChartKind::Line => "lineChart",
            ChartKind::Pie => "pieChart",
        }
    }
}

/// A chart drawn on the first sheet: labels from column A, values from a chosen
/// column, over the data rows.
#[derive(Debug, Clone)]
pub struct Chart {
    pub kind: ChartKind,
    pub title: String,
    /// Zero-based column holding the values (defaults to 1, i.e. column B).
    pub value_column: usize,
}

/// Write delimited `data` (CSV or TSV) to `path` as a single-sheet workbook.
pub fn write_xlsx(path: &Path, data: &str, sheet_name: &str) -> Result<(), String> {
    write_workbook(
        path,
        &[Sheet {
            name: sheet_name.to_string(),
            data: data.to_string(),
        }],
        None,
    )
}

/// Write a workbook of one or more sheets, optionally charting the first one.
pub fn write_workbook(path: &Path, sheets: &[Sheet], chart: Option<&Chart>) -> Result<(), String> {
    if sheets.is_empty() {
        return Err("no sheets to write".to_string());
    }
    let parsed: Vec<(String, Vec<Vec<String>>)> = sheets
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let name = if s.name.trim().is_empty() {
                format!("Sheet{}", i + 1)
            } else {
                sanitize_sheet_name(&s.name)
            };
            (name, parse_delimited(&s.data))
        })
        .collect();
    if parsed.iter().all(|(_, rows)| rows.is_empty()) {
        return Err("no rows to write".to_string());
    }
    // A chart needs at least a header plus one data row.
    let chart = chart.filter(|_| parsed[0].1.len() > 1);

    let mut parts: Vec<(String, String)> = vec![
        (
            "[Content_Types].xml".into(),
            xlsx_content_types(parsed.len(), chart.is_some()),
        ),
        ("_rels/.rels".into(), XLSX_ROOT_RELS.to_string()),
        (
            "xl/_rels/workbook.xml.rels".into(),
            xlsx_workbook_rels(parsed.len()),
        ),
        ("xl/styles.xml".into(), XLSX_STYLES.to_string()),
        (
            "xl/workbook.xml".into(),
            workbook_xml(&parsed.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>()),
        ),
    ];
    for (i, (_, rows)) in parsed.iter().enumerate() {
        // Only the first sheet carries the drawing that hosts the chart.
        let has_chart = chart.is_some() && i == 0;
        parts.push((
            format!("xl/worksheets/sheet{}.xml", i + 1),
            sheet_xml(rows, has_chart),
        ));
    }
    if let Some(chart) = chart {
        let (name, rows) = &parsed[0];
        parts.push((
            "xl/worksheets/_rels/sheet1.xml.rels".into(),
            SHEET_DRAWING_RELS.to_string(),
        ));
        parts.push(("xl/drawings/drawing1.xml".into(), DRAWING.to_string()));
        parts.push((
            "xl/drawings/_rels/drawing1.xml.rels".into(),
            DRAWING_RELS.to_string(),
        ));
        parts.push(("xl/charts/chart1.xml".into(), chart_xml(chart, name, rows)));
    }
    let refs: Vec<(&str, String)> = parts.iter().map(|(n, c)| (n.as_str(), c.clone())).collect();
    zip_parts(path, &refs)
}

/// Excel forbids `[]:*?/\` in sheet names and caps them at 31 characters.
fn sanitize_sheet_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .filter(|c| !['[', ']', ':', '*', '?', '/', '\\'].contains(c))
        .collect();
    cleaned.chars().take(31).collect()
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
fn sheet_xml(rows: &[Vec<String>], has_chart: bool) -> String {
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
         {cols}<sheetData>{body}</sheetData>{drawing}</worksheet>",
        cols = cols_xml(rows),
        // `drawing` must come after sheetData — Excel enforces element order.
        drawing = if has_chart {
            "<drawing xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" r:id=\"rId1\"/>"
        } else {
            ""
        }
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

fn workbook_xml(sheet_names: &[String]) -> String {
    let sheets: String = sheet_names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            format!(
                "<sheet name=\"{}\" sheetId=\"{n}\" r:id=\"rId{n}\"/>",
                xml_escape(name),
                n = i + 1
            )
        })
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <sheets>{sheets}</sheets></workbook>"
    )
}

/// Content types: one override per worksheet, plus the chart/drawing when used.
fn xlsx_content_types(sheets: usize, chart: bool) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
         <Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>",
    );
    for i in 1..=sheets {
        out.push_str(&format!(
            "<Override PartName=\"/xl/worksheets/sheet{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"
        ));
    }
    if chart {
        out.push_str(
            "<Override PartName=\"/xl/drawings/drawing1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.drawing+xml\"/>\
             <Override PartName=\"/xl/charts/chart1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.drawingml.chart+xml\"/>",
        );
    }
    out.push_str("</Types>");
    out
}

/// Workbook relationships: rId1..N are the sheets, then styles.
fn xlsx_workbook_rels(sheets: usize) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );
    for i in 1..=sheets {
        out.push_str(&format!(
            "<Relationship Id=\"rId{i}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{i}.xml\"/>"
        ));
    }
    out.push_str(&format!(
        "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>",
        sheets + 1
    ));
    out
}

/// Build the chart part, pointing its category and value series at real ranges
/// on the first sheet so the chart updates when the numbers do.
fn chart_xml(chart: &Chart, sheet: &str, rows: &[Vec<String>]) -> String {
    let last = rows.len(); // header is row 1, so data ends at row `rows.len()`
    let value_col = column_name(chart.value_column);
    let label_col = column_name(0);
    // Sheet names are single-quoted in formulas; embedded quotes are doubled.
    let quoted = sheet.replace('\'', "''");
    let cats = format!("'{quoted}'!${label_col}$2:${label_col}${last}");
    let vals = format!("'{quoted}'!${value_col}$2:${value_col}${last}");
    let series_name = format!("'{quoted}'!${value_col}$1");

    // Pie charts have no axes; bar and line share a category/value axis pair.
    let (axes_ids, axes) = if chart.kind == ChartKind::Pie {
        (String::new(), String::new())
    } else {
        (
            "<c:axId val=\"111111111\"/><c:axId val=\"222222222\"/>".to_string(),
            "<c:catAx><c:axId val=\"111111111\"/><c:scaling><c:orientation val=\"minMax\"/>\
             </c:scaling><c:delete val=\"0\"/><c:axPos val=\"b\"/>\
             <c:crossAx val=\"222222222\"/></c:catAx>\
             <c:valAx><c:axId val=\"222222222\"/><c:scaling><c:orientation val=\"minMax\"/>\
             </c:scaling><c:delete val=\"0\"/><c:axPos val=\"l\"/>\
             <c:crossAx val=\"111111111\"/></c:valAx>"
                .to_string(),
        )
    };
    let kind_attrs = match chart.kind {
        ChartKind::Bar => "<c:barDir val=\"col\"/><c:grouping val=\"clustered\"/>",
        ChartKind::Line => "<c:grouping val=\"standard\"/>",
        ChartKind::Pie => "",
    };
    let element = chart.kind.element();
    let title = if chart.title.trim().is_empty() {
        String::new()
    } else {
        format!(
            "<c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>{}</a:t></a:r></a:p></c:rich>\
             </c:tx><c:overlay val=\"0\"/></c:title><c:autoTitleDeleted val=\"0\"/>",
            xml_escape(chart.title.trim())
        )
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" \
         xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <c:chart>{title}<c:plotArea><c:layout/>\
         <c:{element}>{kind_attrs}\
         <c:ser><c:idx val=\"0\"/><c:order val=\"0\"/>\
         <c:tx><c:strRef><c:f>{series_name}</c:f></c:strRef></c:tx>\
         <c:cat><c:strRef><c:f>{cats}</c:f></c:strRef></c:cat>\
         <c:val><c:numRef><c:f>{vals}</c:f></c:numRef></c:val>\
         </c:ser>{axes_ids}</c:{element}>{axes}</c:plotArea>\
         <c:plotVisOnly val=\"1\"/><c:dispBlanksAs val=\"gap\"/></c:chart></c:chartSpace>"
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

/// Sheet 1 → the drawing that hosts the chart.
const SHEET_DRAWING_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing\" Target=\"../drawings/drawing1.xml\"/>\
</Relationships>";

/// The drawing → the chart part.
const DRAWING_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart\" Target=\"../charts/chart1.xml\"/>\
</Relationships>";

/// Anchors the chart to the right of the data (columns E–M, rows 2–18).
const DRAWING: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<xdr:wsDr xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\" \
xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
<xdr:twoCellAnchor>\
<xdr:from><xdr:col>4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>\
<xdr:to><xdr:col>12</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>18</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>\
<xdr:graphicFrame macro=\"\">\
<xdr:nvGraphicFramePr><xdr:cNvPr id=\"2\" name=\"Chart 1\"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr>\
<xdr:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/></xdr:xfrm>\
<a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\">\
<c:chart r:id=\"rId1\"/></a:graphicData></a:graphic>\
</xdr:graphicFrame><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>";

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

        let xml = sheet_xml(&rows, false);
        // Text is inline; numbers are stored as numbers so Excel can sum them.
        assert!(xml.contains("t=\"inlineStr\""));
        assert!(xml.contains("<c r=\"B2\"><v>1500</v></c>"));
    }

    #[test]
    fn formulas_header_and_layout() {
        let rows =
            parse_delimited("Region,Revenue\nKampala,1500\nNairobi,900\nTotal,=SUM(B2:B3)\n");
        let xml = sheet_xml(&rows, false);
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
    fn chart_series_point_at_real_sheet_ranges() {
        let rows = parse_delimited("Region,Revenue\nKampala,1500\nNairobi,900\nLagos,1200\n");
        let chart = Chart {
            kind: ChartKind::Bar,
            title: "Revenue by region".to_string(),
            value_column: 1,
        };
        let xml = chart_xml(&chart, "Q3 Revenue", &rows);
        // Categories from column A, values from column B, over the data rows.
        assert!(xml.contains("<c:f>'Q3 Revenue'!$A$2:$A$4</c:f>"), "{xml}");
        assert!(xml.contains("<c:f>'Q3 Revenue'!$B$2:$B$4</c:f>"), "{xml}");
        assert!(xml.contains("<c:f>'Q3 Revenue'!$B$1</c:f>"), "series name");
        assert!(xml.contains("barChart") && xml.contains("Revenue by region"));
        // Bar/line charts need an axis pair; pie charts must not have one.
        assert!(xml.contains("c:catAx"));
        let pie = Chart {
            kind: ChartKind::Pie,
            title: String::new(),
            value_column: 1,
        };
        let pie_xml = chart_xml(&pie, "S", &rows);
        assert!(pie_xml.contains("pieChart"));
        assert!(!pie_xml.contains("c:catAx"), "pie charts have no axes");
    }

    #[test]
    fn chart_kinds_and_sheet_names_are_normalised() {
        assert_eq!(ChartKind::parse("Column"), Some(ChartKind::Bar));
        assert_eq!(ChartKind::parse("trend"), Some(ChartKind::Line));
        assert_eq!(ChartKind::parse("donut"), Some(ChartKind::Pie));
        assert_eq!(ChartKind::parse("radar"), None);
        // Excel rejects these characters and names over 31 chars.
        assert_eq!(sanitize_sheet_name("Q3/Q4: [data]?"), "Q3Q4 data");
        assert_eq!(sanitize_sheet_name(&"x".repeat(40)).len(), 31);
    }

    #[test]
    fn multi_sheet_workbook_wires_every_part() {
        let names = vec!["Data".to_string(), "Summary".to_string()];
        let wb = workbook_xml(&names);
        assert!(wb.contains("name=\"Data\" sheetId=\"1\" r:id=\"rId1\""));
        assert!(wb.contains("name=\"Summary\" sheetId=\"2\" r:id=\"rId2\""));
        // Sheets take rId1..N; styles follow.
        let rels = xlsx_workbook_rels(2);
        assert!(rels.contains("Id=\"rId2\"") && rels.contains("worksheets/sheet2.xml"));
        assert!(rels.contains("Id=\"rId3\"") && rels.contains("styles.xml"));
        // Content types list every sheet, and chart parts only when charted.
        let types = xlsx_content_types(2, true);
        assert!(types.contains("/xl/worksheets/sheet2.xml"));
        assert!(types.contains("/xl/charts/chart1.xml"));
        assert!(!xlsx_content_types(2, false).contains("chart1.xml"));
        // The drawing reference belongs after sheetData, and only when charted.
        assert!(sheet_xml(&[vec!["a".into()]], true).contains("<drawing"));
        assert!(!sheet_xml(&[vec!["a".into()]], false).contains("<drawing"));
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
