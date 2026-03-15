use std::fs;
use std::path::Path;

use litchi::sheet::CellValue;

/// Returns true if the extension is an Office format we extract with litchi.
pub fn is_office_extension(ext: &str) -> bool {
    matches!(
        ext,
        "doc" | "docx" | "xls" | "xlsx" | "xlsb"
    )
}

/// Extract plain text from a file. Returns None if the file is not indexable or extraction fails.
pub fn extract_text(path: &Path) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase());
    let ext_str = ext.as_deref().unwrap_or("");

    if ext_str == "pdf" {
        return extract_pdf(path);
    }
    if is_office_extension(ext_str) {
        extract_office(path, ext_str)
    } else {
        extract_plain_text(path, ext_str)
    }
}

fn extract_pdf(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    pdf_extract::extract_text_from_mem(&bytes).ok()
}

fn extract_plain_text(path: &Path, ext: &str) -> Option<String> {
    let text_exts = [
        "rs", "ts", "tsx", "js", "jsx", "md", "txt", "json", "toml", "yaml", "yml",
    ];
    if ext.is_empty() || text_exts.contains(&ext) {
        fs::read_to_string(path).ok()
    } else {
        None
    }
}

fn extract_office(path: &Path, ext: &str) -> Option<String> {
    match ext {
        "doc" | "docx" => extract_word(path),
        "xls" | "xlsx" | "xlsb" => extract_excel(path, ext),
        _ => None,
    }
}

fn extract_word(path: &Path) -> Option<String> {
    let doc = litchi::Document::open(path).ok()?;
    doc.text().ok()
}

fn extract_excel(path: &Path, ext: &str) -> Option<String> {
    use litchi::sheet::{open_workbook, open_xls_workbook, open_xlsb_workbook, Workbook};

    let workbook: Box<dyn Workbook> = match ext {
        "xlsx" => open_workbook(path).ok()?,
        "xls" => Box::new(open_xls_workbook(path).ok()?),
        "xlsb" => Box::new(open_xlsb_workbook(path).ok()?),
        _ => return None,
    };

    let mut out = String::new();
    let names = workbook.worksheet_names();
    for name in names {
        let worksheet = workbook.worksheet_by_name(&name).ok()?;
        out.push_str("Sheet: ");
        out.push_str(worksheet.name());
        out.push('\n');
        let mut cells = worksheet.cells();
        while let Some(cell_result) = cells.next() {
            let cell = match cell_result {
                Ok(c) => c,
                Err(_) => continue,
            };
            let s = cell_value_to_string(cell.value());
            if !s.is_empty() {
                out.push_str(&s);
                out.push('\t');
            }
        }
        if out.ends_with('\t') {
            out.pop();
        }
        out.push('\n');
    }
    Some(out.trim_end().to_string())
}

fn cell_value_to_string(v: &CellValue) -> String {
    match v {
        CellValue::Empty => String::new(),
        CellValue::Bool(b) => b.to_string(),
        CellValue::Int(i) => i.to_string(),
        CellValue::Float(f) => f.to_string(),
        CellValue::String(s) => s.clone(),
        CellValue::DateTime(f) => f.to_string(),
        CellValue::Error(e) => e.clone(),
    }
}
