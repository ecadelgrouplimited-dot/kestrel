//! Attachments: dropping a screenshot or a PDF straight into the conversation.
//!
//! Two very different paths, deliberately:
//!
//! - **Documents** (PDF, Word, Excel, text) are *extracted to text* and folded
//!   into the prompt. That works with every model, needs no vision support, and
//!   costs a fraction of what an image of the same page would.
//! - **Images** are sent as real vision content blocks, because a screenshot's
//!   meaning is in the pixels.
//!
//! Base64 is hand-rolled rather than pulled in as a dependency — it is twenty
//! lines and keeps Kestrel's "no heavy deps" promise intact.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// An image ready to attach to a model request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageAttachment {
    /// e.g. `image/png` — what the API needs to decode it.
    pub media_type: String,
    pub data_base64: String,
}

/// How an attached file should be handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    /// Send as pixels to a vision model.
    Image,
    /// Extract text and fold it into the prompt.
    Document,
    Unsupported,
}

/// Providers reject very large images; keep well under the common 5MB limit.
const MAX_IMAGE_BYTES: u64 = 4_000_000;
/// How much extracted document text to attach before truncating.
const MAX_DOC_CHARS: usize = 60_000;

/// Decide how to handle a file, from its extension.
pub fn classify(path: &Path) -> AttachmentKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" => AttachmentKind::Image,
        _ if crate::office::kind_for(path).is_some() => AttachmentKind::Document,
        // Anything else readable as text is still worth attaching.
        "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "java" | "c" | "cpp" | "h" | "toml"
        | "ini" | "cfg" | "sql" | "sh" | "ps1" => AttachmentKind::Document,
        _ => AttachmentKind::Unsupported,
    }
}

/// The IANA media type for an image path.
fn media_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

/// Read an image and encode it for a vision request.
pub fn load_image(path: &Path) -> Result<ImageAttachment, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_IMAGE_BYTES {
        return Err(format!(
            "{} is {:.1}MB — too large to attach (limit {:.0}MB). Crop or downscale it first.",
            path.display(),
            size as f64 / 1_000_000.0,
            MAX_IMAGE_BYTES as f64 / 1_000_000.0
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(ImageAttachment {
        media_type: media_type(path).to_string(),
        data_base64: base64_encode(&bytes),
    })
}

/// Extract an attached document as a delimited block for the prompt, so the
/// model can tell the attachment apart from the user's own words.
pub fn document_context(path: &Path) -> Result<String, String> {
    let (mut text, _) = crate::office::read_document(path)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    if text.chars().count() > MAX_DOC_CHARS {
        let cut: String = text.chars().take(MAX_DOC_CHARS).collect();
        text = format!("{cut}\n… [truncated — attachment continues]");
    }
    Ok(format!(
        "--- Attached file: {name} ---\n{text}\n--- end of {name} ---"
    ))
}

/// Standard base64, with padding.
pub fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_vectors() {
        // RFC 4648 test vectors — including every padding case.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        // Binary bytes, including the +/ alphabet tail.
        assert_eq!(base64_encode(&[0xfb, 0xff, 0xfe]), "+//+");
        assert_eq!(base64_encode(&[0x00, 0x00, 0x00]), "AAAA");
    }

    #[test]
    fn files_route_to_the_right_handling() {
        assert_eq!(classify(Path::new("shot.PNG")), AttachmentKind::Image);
        assert_eq!(classify(Path::new("photo.jpeg")), AttachmentKind::Image);
        assert_eq!(classify(Path::new("profile.pdf")), AttachmentKind::Document);
        assert_eq!(classify(Path::new("report.docx")), AttachmentKind::Document);
        assert_eq!(classify(Path::new("data.xlsx")), AttachmentKind::Document);
        assert_eq!(classify(Path::new("notes.md")), AttachmentKind::Document);
        assert_eq!(classify(Path::new("main.rs")), AttachmentKind::Document);
        assert_eq!(classify(Path::new("clip.mp4")), AttachmentKind::Unsupported);
    }

    #[test]
    fn media_types_follow_the_extension() {
        assert_eq!(media_type(Path::new("a.png")), "image/png");
        assert_eq!(media_type(Path::new("a.jpg")), "image/jpeg");
        assert_eq!(media_type(Path::new("a.JPEG")), "image/jpeg");
        assert_eq!(media_type(Path::new("a.webp")), "image/webp");
    }

    #[test]
    fn documents_are_wrapped_so_the_model_sees_the_boundary() {
        let dir = std::env::temp_dir().join(format!("kestrel-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("profile.md");
        std::fs::write(&file, "# ECADEL\n\nAfrica's infrastructure.").unwrap();
        let block = document_context(&file).unwrap();
        assert!(block.starts_with("--- Attached file: profile.md ---"));
        assert!(block.contains("Africa's infrastructure."));
        assert!(block.trim_end().ends_with("--- end of profile.md ---"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_images_are_refused_with_advice() {
        let dir = std::env::temp_dir().join(format!("kestrel-media-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("huge.png");
        std::fs::write(&big, vec![0u8; (MAX_IMAGE_BYTES + 10) as usize]).unwrap();
        let err = load_image(&big).unwrap_err();
        assert!(err.contains("too large") && err.contains("downscale"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_small_image_encodes() {
        let dir = std::env::temp_dir().join(format!("kestrel-media-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("dot.png");
        std::fs::write(&img, b"foobar").unwrap();
        let att = load_image(&img).unwrap();
        assert_eq!(att.media_type, "image/png");
        assert_eq!(att.data_base64, "Zm9vYmFy");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
