use std::collections::BTreeSet;

fn is_memkit_internal(path: &str) -> bool {
    let p = path.replace('\\', "/");
    p.contains("/.memkit/")
        || p.ends_with("/.memkit")
        || p.ends_with("memkit.txt")
        || p.ends_with("manifest.json")
        || p.ends_with("index.json")
        || p.ends_with("file_state.json")
        || p.contains("/lancedb/")
        || p.ends_with(".lance")
}

pub fn format_file_tree(paths: &[String], base_path: &str) -> String {
    let base = base_path.replace('\\', "/").trim_end_matches('/').to_string();
    let base_prefix = if base.is_empty() {
        String::new()
    } else {
        format!("{}/", base)
    };
    let mut entries: BTreeSet<String> = BTreeSet::new();
    for p in paths {
        let normalized = p.replace('\\', "/");
        if is_memkit_internal(&normalized) {
            continue;
        }
        let relative = if normalized.starts_with(&base_prefix) {
            normalized[base_prefix.len()..].to_string()
        } else if normalized.starts_with(&base) {
            normalized[base.len()..].trim_start_matches('/').to_string()
        } else {
            continue;
        };
        if !relative.is_empty() {
            entries.insert(relative);
        }
    }

    let sorted: Vec<_> = entries.into_iter().collect();
    let mut lines = Vec::new();
    for (i, entry) in sorted.iter().enumerate() {
        let is_last = i == sorted.len() - 1;
        let prefix = if is_last { "└── " } else { "├── " };
        lines.push(format!("{}{}", prefix, entry));
    }
    lines.join("\n")
}
