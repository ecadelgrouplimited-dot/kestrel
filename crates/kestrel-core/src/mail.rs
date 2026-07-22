//! Email through the user's own Outlook.
//!
//! Triage an inbox, read a thread, and draft a reply — via Outlook's MAPI
//! objects over PowerShell. This *is* COM, unlike the document modules, because
//! there is no file format to parse: the mail lives in the user's profile.
//!
//! The lessons from the Word/Excel work are applied throughout:
//! - **Never `Quit()`.** Outlook COM returns the user's *running* Outlook;
//!   quitting it would close their mail client. We only read and create items.
//! - **Hard timeout + drained pipes**, so a prompt or a large mailbox can't
//!   wedge a run.
//! - **Drafts are saved, never sent.** Sending is a separate, always-confirmed
//!   action — see `tool_always_needs_permission` in the UI.

use std::process::Command;

/// One inbox entry in a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailSummary {
    pub index: usize,
    pub unread: bool,
    pub received: String,
    pub sender: String,
    pub subject: String,
}

/// Cap on any single Outlook call.
const MAIL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// Characters of a message body to return.
const BODY_CAP: usize = 12_000;

/// List the most recent `count` messages in the Inbox, newest first.
pub fn list_inbox(count: usize) -> Result<Vec<MailSummary>, String> {
    let count = count.clamp(1, 100);
    let script = format!(
        "{PRELUDE}\
         $items = $inbox.Items\n\
         $items.Sort('[ReceivedTime]', $true)\n\
         $max = [Math]::Min({count}, $items.Count)\n\
         for ($i = 1; $i -le $max; $i++) {{\n\
         \x20 try {{\n\
         \x20   $m = $items.Item($i)\n\
         \x20   $flag = if ($m.UnRead) {{ 'UNREAD' }} else {{ 'read' }}\n\
         \x20   $when = $m.ReceivedTime.ToString('yyyy-MM-dd HH:mm')\n\
         \x20   $from = ($m.SenderName -replace '\\t', ' ')\n\
         \x20   $subj = ($m.Subject -replace '\\t', ' ')\n\
         \x20   [Console]::Out.WriteLine(\"$i`t$flag`t$when`t$from`t$subj\")\n\
         \x20 }} catch {{ }}\n\
         }}\n"
    );
    Ok(run_outlook(&script)?
        .lines()
        .filter_map(parse_summary)
        .collect())
}

/// Parse one tab-separated listing line.
fn parse_summary(line: &str) -> Option<MailSummary> {
    let parts: Vec<&str> = line.trim_end().split('\t').collect();
    if parts.len() < 5 {
        return None;
    }
    Some(MailSummary {
        index: parts[0].trim().parse().ok()?,
        unread: parts[1].eq_ignore_ascii_case("UNREAD"),
        received: parts[2].to_string(),
        sender: parts[3].to_string(),
        subject: parts[4].to_string(),
    })
}

/// Read one message from the Inbox by its listing index (1 = newest).
pub fn read_message(index: usize) -> Result<String, String> {
    let index = index.max(1);
    let script = format!(
        "{PRELUDE}\
         $items = $inbox.Items\n\
         $items.Sort('[ReceivedTime]', $true)\n\
         if ({index} -gt $items.Count) {{ throw 'NO_SUCH_MESSAGE' }}\n\
         $m = $items.Item({index})\n\
         [Console]::Out.WriteLine('From: ' + $m.SenderName + ' <' + $m.SenderEmailAddress + '>')\n\
         [Console]::Out.WriteLine('To: ' + $m.To)\n\
         [Console]::Out.WriteLine('Date: ' + $m.ReceivedTime.ToString('yyyy-MM-dd HH:mm'))\n\
         [Console]::Out.WriteLine('Subject: ' + $m.Subject)\n\
         [Console]::Out.WriteLine('')\n\
         [Console]::Out.Write($m.Body)\n"
    );
    let mut body = run_outlook(&script)?;
    if body.len() > BODY_CAP {
        body.truncate(
            (0..=BODY_CAP)
                .rev()
                .find(|&i| body.is_char_boundary(i))
                .unwrap_or(0),
        );
        body.push_str("\n… [message truncated]");
    }
    Ok(body)
}

/// Save a draft to the Drafts folder. **Never sends.**
pub fn draft_message(to: &str, subject: &str, body: &str) -> Result<String, String> {
    if to.trim().is_empty() {
        return Err("a recipient is required".to_string());
    }
    let script = format!(
        "$ErrorActionPreference = 'Stop'\n\
         [Console]::OutputEncoding = [Text.Encoding]::UTF8\n\
         $ol = New-Object -ComObject Outlook.Application\n\
         $mail = $ol.CreateItem(0)\n\
         $mail.To = {to}\n\
         $mail.Subject = {subject}\n\
         $mail.Body = {body}\n\
         $mail.Save()\n\
         [Console]::Out.WriteLine('SAVED')\n",
        to = ps_quote(to),
        subject = ps_quote(subject),
        body = ps_quote(body),
    );
    run_outlook(&script)?;
    Ok(format!("Draft saved to Outlook Drafts, addressed to {to}."))
}

/// Send a message immediately. The caller **must** have confirmed with the user.
pub fn send_message(to: &str, subject: &str, body: &str) -> Result<String, String> {
    if to.trim().is_empty() {
        return Err("a recipient is required".to_string());
    }
    let script = format!(
        "$ErrorActionPreference = 'Stop'\n\
         [Console]::OutputEncoding = [Text.Encoding]::UTF8\n\
         $ol = New-Object -ComObject Outlook.Application\n\
         $mail = $ol.CreateItem(0)\n\
         $mail.To = {to}\n\
         $mail.Subject = {subject}\n\
         $mail.Body = {body}\n\
         $mail.Send()\n\
         [Console]::Out.WriteLine('SENT')\n",
        to = ps_quote(to),
        subject = ps_quote(subject),
        body = ps_quote(body),
    );
    run_outlook(&script)?;
    Ok(format!("Sent to {to}."))
}

/// Opens Outlook's MAPI namespace and the default Inbox (folder 6).
/// Note the deliberate absence of any `Quit()` — see the module docs.
const PRELUDE: &str = "$ErrorActionPreference = 'Stop'\n\
                       [Console]::OutputEncoding = [Text.Encoding]::UTF8\n\
                       $ol = New-Object -ComObject Outlook.Application\n\
                       $ns = $ol.GetNamespace('MAPI')\n\
                       $inbox = $ns.GetDefaultFolder(6)\n";

/// Single-quote a value for PowerShell (doubling embedded quotes).
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Run an Outlook script, translating the common failures to plain language.
fn run_outlook(script: &str) -> Result<String, String> {
    use std::io::Read;
    let file = std::env::temp_dir().join(format!(
        "kestrel-mail-{}-{}.ps1",
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
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
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
                if started.elapsed() > MAIL_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&file);
                    return Err(
                        "Outlook did not respond in time (it may be showing a prompt)".to_string(),
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(80));
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
    Err(explain(&stderr))
}

/// Turn a raw PowerShell/COM error into something a person can act on.
fn explain(stderr: &str) -> String {
    if stderr.contains("NO_SUCH_MESSAGE") {
        return "there is no message at that position in the inbox".to_string();
    }
    if stderr.contains("80040154") || stderr.contains("Retrieving the COM class factory") {
        return "Microsoft Outlook isn't installed on this machine, so mail isn't available"
            .to_string();
    }
    if stderr.contains("MAPI_E_FAILONEPROVIDER") || stderr.contains("profile") {
        return "Outlook is installed but has no configured mail profile — open Outlook and set \
                up an account first"
            .to_string();
    }
    format!("Outlook error: {}", stderr.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_lines_parse_into_summaries() {
        let line = "3\tUNREAD\t2026-07-22 09:14\tWilson Ecaat\tQ3 board pack";
        let m = parse_summary(line).unwrap();
        assert_eq!(m.index, 3);
        assert!(m.unread);
        assert_eq!(m.sender, "Wilson Ecaat");
        assert_eq!(m.subject, "Q3 board pack");

        let read = parse_summary("1\tread\t2026-07-21 08:00\tA\tB").unwrap();
        assert!(!read.unread);
        // Malformed lines are skipped rather than panicking.
        assert!(parse_summary("garbage").is_none());
    }

    #[test]
    fn quoting_protects_against_broken_scripts() {
        let q = ps_quote("Bob's report");
        assert_eq!(q, "'Bob''s report'");
        // Newlines are fine inside single-quoted PowerShell strings.
        assert!(ps_quote("line1\nline2").contains('\n'));
    }

    #[test]
    fn errors_are_translated_for_people() {
        assert!(explain("...80040154...").contains("isn't installed"));
        assert!(explain("NO_SUCH_MESSAGE").contains("no message"));
        assert!(explain("no profile configured").contains("mail profile"));
        assert!(explain("something odd").contains("Outlook error"));
    }

    #[test]
    fn a_draft_needs_a_recipient() {
        assert!(draft_message("", "s", "b")
            .unwrap_err()
            .contains("recipient"));
        assert!(send_message("  ", "s", "b")
            .unwrap_err()
            .contains("recipient"));
    }
}
