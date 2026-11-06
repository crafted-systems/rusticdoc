//! The ceilings a single request runs under.

/// Largest text render, `batchName` or PDF form tag output, unless
/// `RUSTICDOC_MAX_RENDER_BYTES` says otherwise.
pub const DEFAULT_MAX_RENDER_BYTES: usize = 64 * 1024 * 1024;
