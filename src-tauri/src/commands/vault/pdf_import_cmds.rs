use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::vault;

use super::boundary::{with_boundary, with_validated_path, ValidatedPathMode};

const MIN_PAGE_TEXT_CHARS: usize = 80;
const MAX_REPLACEMENT_CHAR_RATIO: f32 = 0.02;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PdfMarkdownOcrMode {
    TextOnly,
    OcrWhenNeeded,
    OcrAllPages,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct PdfMarkdownImportResult {
    pub note_path: String,
    pub note_title: String,
    pub page_count: Option<u32>,
    pub pages_text_extracted: u32,
    pub pages_ocr: u32,
    pub ocr_available: bool,
    pub text_length: usize,
}

#[derive(Debug)]
struct PageMarkdown {
    page_number: u32,
    text: String,
    source: PageTextSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageTextSource {
    Embedded,
    Ocr,
    Empty,
}

struct ExternalToolSet {
    pdfinfo: bool,
    pdftotext: bool,
    pdftoppm: bool,
    tesseract: bool,
}

impl ExternalToolSet {
    fn detect() -> Self {
        Self {
            pdfinfo: command_available("pdfinfo"),
            pdftotext: command_available("pdftotext"),
            pdftoppm: command_available("pdftoppm"),
            tesseract: command_available("tesseract"),
        }
    }

    fn ocr_available(&self) -> bool {
        self.pdftoppm && self.tesseract
    }
}

#[tauri::command]
pub async fn convert_pdf_to_markdown_note(
    pdf_path: PathBuf,
    vault_path: Option<PathBuf>,
    ocr_mode: PdfMarkdownOcrMode,
    ocr_language: Option<String>,
) -> Result<PdfMarkdownImportResult, String> {
    tokio::task::spawn_blocking(move || {
        convert_pdf_to_markdown_note_blocking(pdf_path, vault_path, ocr_mode, ocr_language)
    })
    .await
    .map_err(|error| format!("Task panicked: {error}"))?
}

fn convert_pdf_to_markdown_note_blocking(
    pdf_path: PathBuf,
    vault_path: Option<PathBuf>,
    ocr_mode: PdfMarkdownOcrMode,
    ocr_language: Option<String>,
) -> Result<PdfMarkdownImportResult, String> {
    let raw_vault_path = vault_path_string(vault_path.as_deref());
    let raw_pdf_path = pdf_path.to_string_lossy().to_string();
    let pdf_path = PathBuf::from(with_validated_path(
        &raw_pdf_path,
        raw_vault_path.as_deref(),
        ValidatedPathMode::Existing,
        |validated_path| Ok(validated_path.to_string()),
    )?);
    ensure_pdf_path(&pdf_path)?;

    let tools = ExternalToolSet::detect();
    if !tools.pdftotext && ocr_mode != PdfMarkdownOcrMode::OcrAllPages {
        return Err("PDF text extraction requires pdftotext. Install Poppler to convert this PDF.".to_string());
    }
    if requires_ocr(ocr_mode) && !tools.ocr_available() {
        return Err("OCR requires pdftoppm and tesseract. Install Poppler and Tesseract, or use text extraction only.".to_string());
    }

    let page_count = if tools.pdfinfo {
        pdf_page_count(&pdf_path)?
    } else {
        None
    };
    if requires_ocr(ocr_mode) && page_count.is_none() {
        return Err("OCR requires pdfinfo so Tolaria can process pages safely. Install Poppler and try again.".to_string());
    }

    let pages = extract_pages(&pdf_path, ocr_mode, ocr_language.as_deref(), page_count, &tools)?;
    let title = title_from_pdf_path(&pdf_path);
    let vault_root = with_boundary(raw_vault_path.as_deref(), |boundary| {
        Ok(boundary.requested_root().to_path_buf())
    })?;
    let relative_pdf_path = relative_path_for_markdown(&vault_root, &pdf_path)?;
    let note_path = unique_note_path(&vault_root, &pdf_path)?;
    let content = build_markdown_note(MarkdownNoteInput {
        title: &title,
        source_pdf: &relative_pdf_path,
        mode: ocr_mode,
        imported_at: &Utc::now().to_rfc3339(),
        page_count,
        pages: &pages,
    });

    vault::create_note_content(note_path.to_string_lossy().as_ref(), &content)?;
    Ok(PdfMarkdownImportResult {
        note_path: note_path.to_string_lossy().to_string(),
        note_title: title,
        page_count,
        pages_text_extracted: pages.iter().filter(|page| page.source == PageTextSource::Embedded).count() as u32,
        pages_ocr: pages.iter().filter(|page| page.source == PageTextSource::Ocr).count() as u32,
        ocr_available: tools.ocr_available(),
        text_length: pages.iter().map(|page| page.text.len()).sum(),
    })
}

fn vault_path_string(path: Option<&Path>) -> Option<String> {
    path.map(|value| value.to_string_lossy().to_string())
}

fn ensure_pdf_path(path: &Path) -> Result<(), String> {
    let extension = path.extension().and_then(OsStr::to_str).unwrap_or_default();
    if extension.eq_ignore_ascii_case("pdf") {
        Ok(())
    } else {
        Err("Only PDF files can be converted to Markdown notes.".to_string())
    }
}

fn requires_ocr(mode: PdfMarkdownOcrMode) -> bool {
    mode != PdfMarkdownOcrMode::TextOnly
}

fn command_available(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok()
}

fn command_stdout(program: &str, args: &[String]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("Failed to run {program}: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("{program} failed with status {}", output.status)
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn pdf_page_count(path: &Path) -> Result<Option<u32>, String> {
    let output = command_stdout("pdfinfo", &[path.to_string_lossy().to_string()])?;
    Ok(output.lines().find_map(|line| {
        let value = line.strip_prefix("Pages:")?.trim();
        value.parse::<u32>().ok()
    }))
}

fn extract_pages(
    path: &Path,
    mode: PdfMarkdownOcrMode,
    language: Option<&str>,
    page_count: Option<u32>,
    tools: &ExternalToolSet,
) -> Result<Vec<PageMarkdown>, String> {
    match page_count {
        Some(count) => (1..=count)
            .map(|page| extract_page(path, mode, language, page, tools))
            .collect(),
        None => Ok(vec![PageMarkdown {
            page_number: 1,
            text: extract_all_embedded_text(path)?,
            source: PageTextSource::Embedded,
        }]),
    }
}

fn extract_page(
    path: &Path,
    mode: PdfMarkdownOcrMode,
    language: Option<&str>,
    page_number: u32,
    tools: &ExternalToolSet,
) -> Result<PageMarkdown, String> {
    let embedded = if tools.pdftotext {
        extract_embedded_text_page(path, page_number)?
    } else {
        String::new()
    };
    let use_ocr = match mode {
        PdfMarkdownOcrMode::TextOnly => false,
        PdfMarkdownOcrMode::OcrAllPages => true,
        PdfMarkdownOcrMode::OcrWhenNeeded => !page_text_is_usable(&embedded),
    };

    if use_ocr {
        let text = ocr_page(path, page_number, language)?;
        return Ok(PageMarkdown {
            page_number,
            source: if text.trim().is_empty() { PageTextSource::Empty } else { PageTextSource::Ocr },
            text,
        });
    }

    Ok(PageMarkdown {
        page_number,
        source: if embedded.trim().is_empty() { PageTextSource::Empty } else { PageTextSource::Embedded },
        text: embedded,
    })
}

fn extract_all_embedded_text(path: &Path) -> Result<String, String> {
    command_stdout("pdftotext", &[
        "-layout".to_string(),
        "-enc".to_string(),
        "UTF-8".to_string(),
        path.to_string_lossy().to_string(),
        "-".to_string(),
    ]).map(normalize_extracted_text)
}

fn extract_embedded_text_page(path: &Path, page_number: u32) -> Result<String, String> {
    command_stdout("pdftotext", &[
        "-layout".to_string(),
        "-enc".to_string(),
        "UTF-8".to_string(),
        "-f".to_string(),
        page_number.to_string(),
        "-l".to_string(),
        page_number.to_string(),
        path.to_string_lossy().to_string(),
        "-".to_string(),
    ]).map(normalize_extracted_text)
}

fn ocr_page(path: &Path, page_number: u32, language: Option<&str>) -> Result<String, String> {
    let dir = tempfile::tempdir().map_err(|error| format!("Failed to create OCR temp dir: {error}"))?;
    let prefix = dir.path().join("page");
    command_stdout("pdftoppm", &[
        "-f".to_string(),
        page_number.to_string(),
        "-l".to_string(),
        page_number.to_string(),
        "-r".to_string(),
        "200".to_string(),
        "-png".to_string(),
        path.to_string_lossy().to_string(),
        prefix.to_string_lossy().to_string(),
    ])?;
    let image_path = find_rendered_page(dir.path())?;
    let mut args = vec![
        image_path.to_string_lossy().to_string(),
        "stdout".to_string(),
    ];
    if let Some(lang) = language.filter(|value| !value.trim().is_empty()) {
        args.push("-l".to_string());
        args.push(lang.trim().to_string());
    }
    command_stdout("tesseract", &args).map(normalize_extracted_text)
}

fn find_rendered_page(dir: &Path) -> Result<PathBuf, String> {
    fs::read_dir(dir)
        .map_err(|error| format!("Failed to read OCR temp dir: {error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(OsStr::to_str).is_some_and(|ext| ext.eq_ignore_ascii_case("png")))
        .ok_or_else(|| "PDF page rendering did not produce an image for OCR.".to_string())
}

fn normalize_extracted_text(text: String) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn page_text_is_usable(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.chars().count() < MIN_PAGE_TEXT_CHARS {
        return false;
    }
    let total = trimmed.chars().count();
    let replacement = trimmed.chars().filter(|ch| *ch == '\u{fffd}').count();
    (replacement as f32 / total as f32) <= MAX_REPLACEMENT_CHAR_RATIO
}

fn title_from_pdf_path(path: &Path) -> String {
    path.file_stem()
        .and_then(OsStr::to_str)
        .map(title_from_stem)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| "Imported PDF".to_string())
}

fn title_from_stem(stem: &str) -> String {
    stem.replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn markdown_link_target(path: &str) -> String {
    if path.contains(char::is_whitespace) {
        format!("<{}>", path.replace('>', "%3E"))
    } else {
        path.to_string()
    }
}

fn relative_path_for_markdown(root: &Path, path: &Path) -> Result<String, String> {
    path.strip_prefix(root)
        .map_err(|_| "PDF path must stay inside the vault.".to_string())
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

fn unique_note_path(root: &Path, pdf_path: &Path) -> Result<PathBuf, String> {
    let parent = pdf_path.parent().unwrap_or(root);
    let stem = pdf_path.file_stem().and_then(OsStr::to_str).unwrap_or("Imported PDF");
    for index in 0..1000 {
        let filename = if index == 0 {
            format!("{stem}.md")
        } else {
            format!("{stem} {index}.md")
        };
        let candidate = parent.join(filename);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("Could not find an available Markdown filename for the imported PDF.".to_string())
}

struct MarkdownNoteInput<'a> {
    title: &'a str,
    source_pdf: &'a str,
    mode: PdfMarkdownOcrMode,
    imported_at: &'a str,
    page_count: Option<u32>,
    pages: &'a [PageMarkdown],
}

fn build_markdown_note(input: MarkdownNoteInput<'_>) -> String {
    let pages = input.page_count.map_or_else(|| "null".to_string(), |value| value.to_string());
    let pages_text_extracted = input.pages.iter().filter(|page| page.source == PageTextSource::Embedded).count();
    let pages_ocr = input.pages.iter().filter(|page| page.source == PageTextSource::Ocr).count();
    let mut markdown = format!(
        "---\ntype: Note\nsource_pdf: \"{}\"\npdf_import:\n  mode: {}\n  imported_at: \"{}\"\n  pages: {}\n  pages_text_extracted: {}\n  pages_ocr: {}\n---\n\n# {}\n\n[Source PDF]({})\n",
        yaml_escape(input.source_pdf),
        ocr_mode_key(input.mode),
        yaml_escape(input.imported_at),
        pages,
        pages_text_extracted,
        pages_ocr,
        input.title,
        markdown_link_target(input.source_pdf),
    );

    for page in input.pages {
        markdown.push_str(&format!("\n## Page {}\n\n", page.page_number));
        if page.text.trim().is_empty() {
            markdown.push_str("_No text extracted from this page._\n");
        } else {
            markdown.push_str(page.text.trim());
            markdown.push('\n');
        }
    }
    markdown
}

fn yaml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn ocr_mode_key(mode: PdfMarkdownOcrMode) -> &'static str {
    match mode {
        PdfMarkdownOcrMode::TextOnly => "text_only",
        PdfMarkdownOcrMode::OcrWhenNeeded => "ocr_when_needed",
        PdfMarkdownOcrMode::OcrAllPages => "ocr_all_pages",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_quality_rejects_short_or_corrupt_text() {
        assert!(!page_text_is_usable("short"));
        assert!(!page_text_is_usable(&format!("{}{}", "a".repeat(100), "\u{fffd}".repeat(5))));
        assert!(page_text_is_usable(&"Readable extracted paragraph. ".repeat(8)));
    }

    #[test]
    fn markdown_links_wrap_paths_with_spaces() {
        assert_eq!(markdown_link_target("attachments/report.pdf"), "attachments/report.pdf");
        assert_eq!(markdown_link_target("attachments/project brief.pdf"), "<attachments/project brief.pdf>");
    }

    #[test]
    fn generated_markdown_keeps_pdf_source_and_pages() {
        let pages = vec![
            PageMarkdown { page_number: 1, text: "Hello".to_string(), source: PageTextSource::Embedded },
            PageMarkdown { page_number: 2, text: "Scanned".to_string(), source: PageTextSource::Ocr },
        ];
        let markdown = build_markdown_note(MarkdownNoteInput {
            title: "Project Brief",
            source_pdf: "attachments/project brief.pdf",
            mode: PdfMarkdownOcrMode::OcrWhenNeeded,
            imported_at: "2026-06-06T12:00:00Z",
            page_count: Some(2),
            pages: &pages,
        });

        assert!(markdown.contains("source_pdf: \"attachments/project brief.pdf\""));
        assert!(markdown.contains("mode: ocr_when_needed"));
        assert!(markdown.contains("pages_text_extracted: 1"));
        assert!(markdown.contains("pages_ocr: 1"));
        assert!(markdown.contains("[Source PDF](<attachments/project brief.pdf>)"));
        assert!(markdown.contains("## Page 2\n\nScanned"));
    }

    #[test]
    fn note_path_uses_pdf_folder_and_avoids_collision() {
        let dir = tempfile::TempDir::new().unwrap();
        let pdf = dir.path().join("reports/Project Brief.pdf");
        fs::create_dir_all(pdf.parent().unwrap()).unwrap();
        fs::write(pdf.parent().unwrap().join("Project Brief.md"), "# Existing\n").unwrap();

        let note = unique_note_path(dir.path(), &pdf).unwrap();

        assert_eq!(note.file_name().and_then(OsStr::to_str), Some("Project Brief 1.md"));
        assert_eq!(note.parent(), pdf.parent());
    }
}
