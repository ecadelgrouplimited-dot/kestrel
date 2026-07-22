//! Reading real office documents.
//!
//! `read_file` only handles UTF-8 text, so a Documents folder full of `.docx`,
//! `.xlsx`, and `.pdf` is invisible to the agent. This module reads them
//! **directly**: Open XML files (`.docx`/`.xlsx`/`.odt`) are ZIP archives of
//! XML, so we pull the parts out through PowerShell's built-in
//! `System.IO.Compression` and turn the XML into text here in Rust.
//!
//! Deliberately **not** Office COM automation. COM attaches to the user's
//! *running* Word/Excel — hiding their window, mutating their session, and
//! hanging forever on modal dialogs (Word's PDF import is notorious). Reading
//! the container directly needs no Office installed, never touches an open
//! document, and cannot hang on a dialog.

use std::path::Path;
use std::process::Command;

/// How a document's text was obtained, for the caller to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    /// Plain text read directly.
    Text,
    /// A Word/OpenDocument file unzipped and de-XML'd.
    Word,
    /// A spreadsheet unzipped and flattened to TSV.
    Excel,
    /// A PDF extracted with `pdftotext`.
    Pdf,
    /// Rich Text Format, de-controlled.
    Rtf,
}

/// Hard cap on an extraction, so nothing can wedge a run.
const EXTRACT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);
/// Rows per sheet before truncating.
const MAX_SHEET_ROWS: usize = 400;

/// Which extraction route a file extension needs, if any.
pub fn kind_for(path: &Path) -> Option<DocKind> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    Some(match ext.as_str() {
        "txt" | "md" | "markdown" | "csv" | "tsv" | "json" | "xml" | "html" | "htm" | "log"
        | "yml" | "yaml" | "rst" => DocKind::Text,
        "docx" | "odt" => DocKind::Word,
        "xlsx" | "xlsm" => DocKind::Excel,
        "pdf" => DocKind::Pdf,
        "rtf" => DocKind::Rtf,
        _ => return None,
    })
}

/// Extract a document's text.
pub fn read_document(path: &Path) -> Result<(String, DocKind), String> {
    if !path.exists() {
        return Err(format!("no such file: {}", path.display()));
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // Legacy binary formats aren't containers we can read — say so plainly.
    if matches!(ext.as_str(), "doc" | "xls" | "ppt") {
        return Err(format!(
            "`.{ext}` is the legacy binary format, which can't be read directly. Open it and \
             save as .{}x, then try again.",
            ext
        ));
    }
    let kind = kind_for(path).ok_or_else(|| {
        format!(
            "unsupported document type: {}",
            path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("(no extension)")
        )
    })?;
    let text = match kind {
        DocKind::Text => std::fs::read_to_string(path).map_err(|e| e.to_string())?,
        DocKind::Rtf => rtf_to_text(&std::fs::read_to_string(path).map_err(|e| e.to_string())?),
        DocKind::Pdf => read_pdf(path)?,
        DocKind::Word => {
            // .docx keeps the body in word/document.xml; .odt in content.xml.
            let entry = if ext == "odt" {
                "content.xml"
            } else {
                "word/document.xml"
            };
            xml_to_text(&zip_entry_text(path, entry)?)
        }
        DocKind::Excel => read_spreadsheet(path)?,
    };
    Ok((text, kind))
}

/// Read one entry out of a ZIP container as text, via PowerShell's built-in
/// compression support (no Office, no extra crates).
fn zip_entry_text(path: &Path, entry: &str) -> Result<String, String> {
    let script = format!(
        "{OPEN_ZIP_PRELUDE}\
         try {{\n\
         \x20 $e = $zip.Entries | Where-Object {{ $_.FullName -eq {entry} }} | Select-Object -First 1\n\
         \x20 if ($e -eq $null) {{ throw 'MISSING_ENTRY' }}\n\
         \x20 $r = New-Object IO.StreamReader($e.Open())\n\
         \x20 [Console]::Out.Write($r.ReadToEnd())\n\
         \x20 $r.Close()\n\
         }} finally {{ $zip.Dispose(); $fs.Dispose() }}\n",
        OPEN_ZIP_PRELUDE = open_zip_prelude(path),
        entry = ps_quote(entry),
    );
    run_powershell(&script)
}

/// PowerShell that opens a container for reading **even while another program
/// holds it open** — `FileShare::ReadWrite` is what lets us read a `.docx` that
/// is currently open in Word, which `ZipFile::OpenRead` cannot do.
fn open_zip_prelude(path: &Path) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         Add-Type -AssemblyName System.IO.Compression\n\
         Add-Type -AssemblyName System.IO.Compression.FileSystem\n\
         $fs = New-Object IO.FileStream({p}, [IO.FileMode]::Open, [IO.FileAccess]::Read, \
         [IO.FileShare]::ReadWrite)\n\
         $zip = New-Object IO.Compression.ZipArchive($fs, [IO.Compression.ZipArchiveMode]::Read)\n",
        p = ps_quote(&path.display().to_string()),
    )
}

/// List the entry names inside a ZIP container.
fn zip_entry_names(path: &Path) -> Result<Vec<String>, String> {
    let script = format!(
        "{prelude}\
         try {{ $zip.Entries | ForEach-Object {{ [Console]::Out.WriteLine($_.FullName) }} }}\n\
         finally {{ $zip.Dispose(); $fs.Dispose() }}\n",
        prelude = open_zip_prelude(path),
    );
    Ok(run_powershell(&script)?
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Flatten a workbook to TSV, one block per sheet.
fn read_spreadsheet(path: &Path) -> Result<String, String> {
    let names = zip_entry_names(path)?;
    let shared = if names.iter().any(|n| n == "xl/sharedStrings.xml") {
        shared_strings(&zip_entry_text(path, "xl/sharedStrings.xml")?)
    } else {
        Vec::new()
    };
    let mut sheets: Vec<&String> = names
        .iter()
        .filter(|n| n.starts_with("xl/worksheets/sheet") && n.ends_with(".xml"))
        .collect();
    sheets.sort();
    if sheets.is_empty() {
        return Err("that workbook has no readable sheets".to_string());
    }
    // Sheet names, in workbook order, when we can read them.
    let titles = zip_entry_text(path, "xl/workbook.xml")
        .map(|xml| sheet_titles(&xml))
        .unwrap_or_default();

    let mut out = String::new();
    for (i, sheet) in sheets.iter().enumerate() {
        let title = titles
            .get(i)
            .cloned()
            .unwrap_or_else(|| format!("Sheet{}", i + 1));
        out.push_str(&format!("## Sheet: {title}\n"));
        out.push_str(&sheet_to_tsv(&zip_entry_text(path, sheet)?, &shared));
        out.push('\n');
    }
    Ok(out)
}

/// Parse `xl/sharedStrings.xml` into the shared-string table.
fn shared_strings(xml: &str) -> Vec<String> {
    xml.split("<si")
        .skip(1)
        .map(|si| {
            let body = si.split_once('>').map(|(_, rest)| rest).unwrap_or(si);
            xml_to_text(body).trim().to_string()
        })
        .collect()
}

/// Sheet display names from `xl/workbook.xml`, in order.
fn sheet_titles(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in xml.split("<sheet ").skip(1) {
        if let Some(rest) = chunk.split_once("name=\"") {
            if let Some((name, _)) = rest.1.split_once('"') {
                out.push(xml_decode(name));
            }
        }
    }
    out
}

/// Turn one worksheet's XML into TSV rows, resolving shared strings.
fn sheet_to_tsv(xml: &str, shared: &[String]) -> String {
    let mut out = String::new();
    let mut rows = 0usize;
    let mut truncated = 0usize;
    for row in xml.split("<row").skip(1) {
        let row = row.split("</row>").next().unwrap_or(row);
        if rows >= MAX_SHEET_ROWS {
            truncated += 1;
            continue;
        }
        let mut cells: Vec<String> = Vec::new();
        for cell in row.split("<c ").skip(1) {
            let head = cell.split_once('>').map(|(h, _)| h).unwrap_or("");
            let is_shared = head.contains("t=\"s\"");
            let body = cell.split_once('>').map(|(_, b)| b).unwrap_or("");
            // A cell's value is <v>…</v>, or inline text in <is>…</is>.
            let raw = body
                .split_once("<v>")
                .and_then(|(_, v)| v.split_once("</v>"))
                .map(|(v, _)| v.to_string())
                .or_else(|| {
                    body.split_once("<is>")
                        .and_then(|(_, v)| v.split_once("</is>"))
                        .map(|(v, _)| xml_to_text(v))
                })
                .or_else(|| {
                    // A formula with no cached value (Excel computes it on open):
                    // show the formula so the reader isn't a blank cell.
                    body.split_once("<f>")
                        .and_then(|(_, f)| f.split_once("</f>"))
                        .map(|(f, _)| format!("={}", xml_decode(f)))
                })
                .unwrap_or_default();
            let value = if is_shared {
                raw.trim()
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| shared.get(i).cloned())
                    .unwrap_or(raw)
            } else {
                xml_decode(&raw)
            };
            cells.push(value.replace(['\t', '\n'], " "));
        }
        out.push_str(&cells.join("\t"));
        out.push('\n');
        rows += 1;
    }
    if truncated > 0 {
        out.push_str(&format!("… ({truncated} more rows)\n"));
    }
    out
}

/// Convert document XML to readable text: paragraphs and breaks become
/// newlines, tabs become tabs, every other tag is dropped.
pub fn xml_to_text(xml: &str) -> String {
    let spaced = xml
        .replace("</w:p>", "\n")
        .replace("<w:tab/>", "\t")
        .replace("<w:br/>", "\n")
        .replace("</text:p>", "\n")
        .replace("</text:h>", "\n");
    let mut out = String::with_capacity(spaced.len() / 2);
    let mut in_tag = false;
    for c in spaced.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    // Collapse the runs of blank lines Open XML tends to produce.
    let decoded = xml_decode(&out);
    let mut result = String::with_capacity(decoded.len());
    let mut blanks = 0;
    for line in decoded.lines() {
        if line.trim().is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        result.push_str(line.trim_end());
        result.push('\n');
    }
    result.trim().to_string()
}

/// Decode the XML entities that appear in Open XML text.
fn xml_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
}

/// Strip RTF control words to leave readable text.
fn rtf_to_text(rtf: &str) -> String {
    let mut out = String::with_capacity(rtf.len() / 2);
    let mut chars = rtf.chars().peekable();
    let mut depth = 0i32;
    while let Some(c) = chars.next() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            '\\' => {
                // A control word runs to the first non-alphanumeric character.
                let mut word = String::new();
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_alphanumeric() || n == '-' {
                        word.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if word.starts_with("par") {
                    out.push('\n');
                }
                // Swallow one space delimiter after a control word.
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
            }
            _ if depth >= 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Extract a PDF's text with `pdftotext` (poppler) if it's on PATH.
fn read_pdf(path: &Path) -> Result<String, String> {
    let out = Command::new("pdftotext")
        .arg("-layout")
        .arg(path)
        .arg("-")
        .output()
        .map_err(|_| {
            "reading PDFs needs `pdftotext` (from Poppler), which isn't installed. Install it \
             (e.g. `winget install --id oschwartz10612.Poppler`) and try again, or convert the \
             PDF to Word/text first."
                .to_string()
        })?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "pdftotext could not read that PDF: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Single-quote a value for PowerShell (doubling any embedded quote).
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Run a PowerShell script from a temp file, killed if it exceeds the timeout.
fn run_powershell(script: &str) -> Result<String, String> {
    let file = std::env::temp_dir().join(format!(
        "kestrel-office-{}-{}.ps1",
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

    // Drain both pipes on threads. A document's XML is far larger than the pipe
    // buffer, so polling alone would deadlock: PowerShell blocks on a full pipe
    // and never exits.
    use std::io::Read;
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = out_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });
    let err_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(pipe) = err_pipe.as_mut() {
            let _ = pipe.read_to_string(&mut buf);
        }
        buf
    });

    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() > EXTRACT_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&file);
                    return Err("reading that document timed out".to_string());
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
        return Ok(String::from_utf8_lossy(&stdout).into_owned());
    }
    let err = stderr;
    if err.contains("MISSING_ENTRY") {
        return Err("that file isn't a readable Open XML document".to_string());
    }
    Err(format!("could not read the document: {}", err.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_extensions_to_the_right_reader() {
        assert_eq!(kind_for(Path::new("notes.md")), Some(DocKind::Text));
        assert_eq!(kind_for(Path::new("report.DOCX")), Some(DocKind::Word));
        assert_eq!(kind_for(Path::new("scan.pdf")), Some(DocKind::Pdf));
        assert_eq!(kind_for(Path::new("budget.xlsx")), Some(DocKind::Excel));
        assert_eq!(kind_for(Path::new("clip.mp4")), None);
    }

    #[test]
    fn word_xml_becomes_readable_text() {
        let xml = "<w:document><w:body>\
                   <w:p><w:r><w:t>ECADEL GROUP</w:t></w:r></w:p>\
                   <w:p><w:r><w:t>Africa&amp;#39;s infrastructure</w:t></w:r></w:p>\
                   <w:p><w:r><w:t>Kampala</w:t></w:r><w:tab/><w:r><w:t>Uganda</w:t></w:r></w:p>\
                   </w:body></w:document>";
        let text = xml_to_text(xml);
        assert!(text.contains("ECADEL GROUP"));
        assert!(text.contains("Kampala\tUganda"));
        // Paragraphs become separate lines.
        assert_eq!(text.lines().count(), 3);
    }

    #[test]
    fn shared_strings_and_cells_resolve_to_tsv() {
        let shared = shared_strings(
            "<sst><si><t>Region</t></si><si><t>Revenue</t></si><si><t>Kampala</t></si></sst>",
        );
        assert_eq!(shared, vec!["Region", "Revenue", "Kampala"]);

        let sheet = "<worksheet><sheetData>\
                     <row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c><c r=\"B1\" t=\"s\"><v>1</v></c></row>\
                     <row r=\"2\"><c r=\"A2\" t=\"s\"><v>2</v></c><c r=\"B2\"><v>1500</v></c></row>\
                     </sheetData></worksheet>";
        let tsv = sheet_to_tsv(sheet, &shared);
        assert_eq!(tsv, "Region\tRevenue\nKampala\t1500\n");
    }

    #[test]
    fn formula_cells_without_a_cached_value_show_the_formula() {
        // Excel computes formulas on open, so a freshly written sheet has <f>
        // but no <v>; the reader must show the formula, not a blank.
        let sheet = "<sheetData><row r=\"5\">\
                     <c r=\"A5\"><f>SUM(B2:B4)</f></c></row></sheetData>";
        assert_eq!(sheet_to_tsv(sheet, &[]), "=SUM(B2:B4)\n");
    }

    #[test]
    fn sheet_titles_are_read_in_order() {
        let xml = "<workbook><sheets>\
                   <sheet name=\"Summary\" sheetId=\"1\" r:id=\"rId1\"/>\
                   <sheet name=\"Q3 Data\" sheetId=\"2\" r:id=\"rId2\"/>\
                   </sheets></workbook>";
        assert_eq!(sheet_titles(xml), vec!["Summary", "Q3 Data"]);
    }

    #[test]
    fn rtf_control_words_are_stripped() {
        let rtf = r"{\rtf1\ansi\deff0 Hello\par World}";
        let text = rtf_to_text(rtf);
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(!text.contains("rtf1"));
    }

    #[test]
    fn plain_text_is_read_directly() {
        let dir = std::env::temp_dir().join(format!("kestrel-office-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note.md");
        std::fs::write(&file, "# Title\n\nbody").unwrap();
        let (text, kind) = read_document(&file).unwrap();
        assert_eq!(kind, DocKind::Text);
        assert!(text.contains("# Title"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_unsupported_and_legacy_files_explain_themselves() {
        assert!(read_document(Path::new("Z:/nope/none.docx"))
            .unwrap_err()
            .contains("no such file"));

        let dir = std::env::temp_dir().join(format!("kestrel-office-x-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("clip.mp4");
        std::fs::write(&clip, "x").unwrap();
        assert!(read_document(&clip).unwrap_err().contains("unsupported"));

        let legacy = dir.join("old.doc");
        std::fs::write(&legacy, "x").unwrap();
        assert!(read_document(&legacy).unwrap_err().contains("legacy"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths_are_quoted_safely_for_powershell() {
        let q = ps_quote(r"C:\Users\Bob's Files\a.docx");
        assert!(q.starts_with('\'') && q.ends_with('\''));
        assert!(q.contains("Bob''s"), "embedded quote must be doubled: {q}");
    }
}
