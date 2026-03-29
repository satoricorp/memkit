//! Input hardening for agent-invoked CLI. Rejects adversarial inputs that agents
//! may hallucinate: path traversal, embedded query params, control chars, etc.

use std::path::PathBuf;

use anyhow::{Result, anyhow};

/// Rejects ASCII control characters (0x00-0x1F) and DEL (0x7F).
pub fn reject_control_chars(s: &str) -> Result<()> {
    for c in s.chars() {
        if c.is_control() {
            return Err(anyhow!(
                "invalid input: control character (U+{:04X}) not allowed",
                c as u32
            ));
        }
    }
    Ok(())
}

/// Validates path-like inputs for sources, ontology, etc.
/// Rejects: `..`, path traversal, control chars, `%` (pre-encoded).
pub fn validate_path(s: &str) -> Result<PathBuf> {
    reject_control_chars(s)?;
    if s.contains('%') {
        return Err(anyhow!(
            "invalid path: '%' not allowed (possible pre-encoded string)"
        ));
    }
    let p = PathBuf::from(s);
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(anyhow!("invalid path: '..' traversal not allowed"));
    }
    Ok(p)
}
