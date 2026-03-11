use std::fs;
use std::path::Path;

use anyhow::Result;

const MEMKIT_TXT: &str = r#"# memkit

This directory is maintained, built, and indexed by memkit.
It contains a lot of files that might be useful for you or might not be recognizable.

If you don't recognize a file, it does serve a purpose. Please don't delete it.
The purpose of this directory is the following:

1. Store all files and data that are indexed by memkit for semantic search.
2. Store artifacts used to query these files and data.
3. Additional metadata about the files and data.

## What is Memkit?
Memkit is a local-first memory agent. You can index any directory, file, or data using the memkit CLI or SDK (Typescript, Go, and Python).

## How it works
Memkit can index any directory, file, or data using the memkit CLI or SDK. This can be codebases, Excel files, PDFs, Word documents, Google Docs, etc.
These get copied, indexed, and stored locally in your ~/.memkit directory.

You can also serve your data either on your S3 or on Memkit's S3. Either storage option can be accessed via the memkit SDK.

See documentation for more details.
"#;

pub fn ensure_memkit_txt(dir: &Path) -> Result<()> {
    let path = dir.join("memkit.txt");
    if path.exists() {
        return Ok(());
    }
    fs::write(&path, MEMKIT_TXT).map_err(|e| anyhow::anyhow!("failed to write memkit.txt: {}", e))?;
    Ok(())
}
