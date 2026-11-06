//! Fixtures and lopdf operations for `tests/pdfops-container.sh`, which
//! cross-checks the engines against the real qpdf, pdfcpu and veraPDF.
//!
//! ```text
//! pdfops_fixtures write <dir>                 write the test fixtures
//! pdfops_fixtures metadata <in> <out> <json>  write metadata with lopdf
//! pdfops_fixtures stamp <in> <out> <text>     stamp text with lopdf
//! pdfops_fixtures merge <out> <in>...         merge with lopdf
//! ```

use std::path::Path;
use std::time::Duration;

use rusticdoc_core::Document;
use rusticdoc_core::pdfops::{LopdfEngine, Overlay, OverlayKind, PdfEngine, fixtures};

fn read(path: &str) -> Document {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let name = Path::new(path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    Document::new(name, bytes)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let timeout = Duration::from_secs(60);
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["write", dir] => fixtures::write_all(Path::new(dir)).map(|_| None),
        ["metadata", input, out, json] => {
            let metadata = serde_json::from_str(json).expect("metadata JSON object");
            LopdfEngine::default()
                .write_metadata(&read(input), &metadata, timeout)
                .await
                .map(|d| Some((out.to_string(), d)))
                .map_err(std::io::Error::other)
        }
        ["stamp", input, out, text] => {
            let overlay = Overlay {
                placement: None,
                kind: OverlayKind::Text {
                    text: text.to_string(),
                    font_size: 24.0,
                    colour: "#808080".into(),
                },
                ..serde_json::from_str(r#"{"type":"text","text":""}"#).unwrap()
            };
            LopdfEngine::default()
                .stamp(&read(input), &overlay, timeout)
                .await
                .map(|d| Some((out.to_string(), d)))
                .map_err(std::io::Error::other)
        }
        ["merge", out, inputs @ ..] if !inputs.is_empty() => {
            let docs: Vec<Document> = inputs.iter().map(|p| read(p)).collect();
            LopdfEngine::default()
                .merge(&docs, timeout)
                .await
                .map(|d| Some((out.to_string(), d)))
                .map_err(std::io::Error::other)
        }
        _ => {
            eprintln!(
                "usage: pdfops_fixtures write <dir> | metadata <in> <out> <json> | stamp <in> <out> <text> | merge <out> <in>..."
            );
            std::process::exit(2);
        }
    };
    match result {
        Ok(Some((out, doc))) => {
            std::fs::write(&out, &doc.bytes).unwrap_or_else(|e| panic!("{out}: {e}"))
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
