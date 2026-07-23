// Embedding the Kestrel mark and version metadata into a Windows executable.
//
// `include!`d by each binary crate's `build.rs` rather than published as a
// crate — build scripts can't share a normal dependency without pulling in a
// whole extra package, and this is a hundred lines of `rc.exe` plumbing. (Plain
// `//` comments, not `//!`: an inner doc comment is illegal once this text is
// spliced into the middle of another file.)
//
// Everything here is best-effort: if the Windows SDK's resource compiler isn't
// installed, the build emits a warning and produces a working binary with the
// default Rust icon. A missing icon must never fail someone's build.

use std::path::{Path, PathBuf};

/// Compile `ico` into a resource and link it into this crate's binaries.
///
/// `exe_name` and `description` land in the file's Properties dialog.
fn embed_icon(ico: &Path, exe_name: &str, description: &str) {
    println!("cargo:rerun-if-changed={}", ico.display());
    // Cross-compiling to a non-Windows target: nothing to embed.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if !ico.exists() {
        println!("cargo:warning=icon not found at {}", ico.display());
        return;
    }
    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found; building without the Kestrel icon");
        return;
    };

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    // 0.1.0 -> 0,1,0,0, the four-part form VERSIONINFO wants.
    let mut parts: Vec<String> = version.split('.').map(|p| p.to_string()).collect();
    parts.resize(4, "0".to_string());
    let quad = parts.join(",");

    // rc.exe accepts forward slashes, which saves escaping every backslash.
    let ico_path = ico.display().to_string().replace('\\', "/");
    let script = format!(
        r#"1 ICON "{ico_path}"

1 VERSIONINFO
FILEVERSION {quad}
PRODUCTVERSION {quad}
FILEOS 0x4
FILETYPE 0x1
{{
  BLOCK "StringFileInfo"
  {{
    BLOCK "040904B0"
    {{
      VALUE "CompanyName", "Kestrel"
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "{exe_name}"
      VALUE "OriginalFilename", "{exe_name}.exe"
      VALUE "ProductName", "Kestrel"
      VALUE "ProductVersion", "{version}"
    }}
  }}
  BLOCK "VarFileInfo"
  {{
    VALUE "Translation", 0x409, 1200
  }}
}}
"#
    );

    let rc_file = out.join("kestrel.rc");
    let res_file = out.join("kestrel.res");
    // rc.exe reads UTF-8 as the system ANSI codepage and mangles anything above
    // ASCII (the em dash in the description turns to mojibake in the file's
    // Properties dialog). A UTF-16LE BOM makes it decode correctly.
    let mut encoded = vec![0xFF, 0xFE];
    for unit in script.encode_utf16() {
        encoded.extend_from_slice(&unit.to_le_bytes());
    }
    if std::fs::write(&rc_file, encoded).is_err() {
        println!("cargo:warning=could not write {}", rc_file.display());
        return;
    }
    let status = std::process::Command::new(&rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_file)
        .arg(&rc_file)
        .status();
    match status {
        Ok(s) if s.success() => {
            // link.exe takes a .res straight on the command line.
            println!("cargo:rustc-link-arg-bins={}", res_file.display());
        }
        Ok(s) => println!("cargo:warning=rc.exe failed ({s}); building without the icon"),
        Err(e) => println!("cargo:warning=could not run rc.exe ({e}); building without the icon"),
    }
}

/// Locate the Windows SDK resource compiler.
///
/// It's on `PATH` only inside a Developer Command Prompt, so fall back to the
/// versioned SDK layout and take the newest one.
fn find_rc() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("KESTREL_RC") {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return Some(path);
        }
    }
    if std::process::Command::new("rc.exe")
        .arg("/?")
        .output()
        .is_ok()
    {
        return Some(PathBuf::from("rc.exe"));
    }

    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86") => "x86",
        Ok("aarch64") => "arm64",
        _ => "x64",
    };
    let mut candidates = Vec::new();
    for root in ["ProgramFiles(x86)", "ProgramFiles"] {
        let Ok(base) = std::env::var(root) else {
            continue;
        };
        let bin = PathBuf::from(base).join("Windows Kits/10/bin");
        let Ok(entries) = std::fs::read_dir(&bin) else {
            continue;
        };
        for entry in entries.flatten() {
            let candidate = entry.path().join(arch).join("rc.exe");
            if candidate.exists() {
                candidates.push(candidate);
            }
        }
    }
    // Directory names are SDK versions, so the last one sorted is the newest.
    candidates.sort();
    candidates.pop()
}
