use std::fs;
use std::path::Path;

use anyhow::Result;

const MEMKIT_TXT: &str = r#"# memkit

This directory is indexed by memkit for semantic search and graph.

## What is memkit

memkit is a local memory pack—it indexes text files for semantic search and graph exploration.

## How it works

memkit reads supported extensions (md, txt, rs, ts, js, json, etc.), chunks, embeds, and stores data in `.memkit/`.

## This directory

[Describe what makes this directory unique—notes, projects, or context.]

## Notes

- memkit only reads recognized text files
- Do not modify files memkit doesn't index
- `.memkit/` is internal—do not edit
"#;

pub fn ensure_memkit_txt(dir: &Path) -> Result<()> {
    let path = dir.join("memkit.txt");
    if path.exists() {
        return Ok(());
    }
    fs::write(&path, MEMKIT_TXT).map_err(|e| anyhow::anyhow!("failed to write memkit.txt: {}", e))?;
    Ok(())
}
