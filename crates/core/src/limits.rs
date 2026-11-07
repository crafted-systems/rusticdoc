//! The ceilings a single request runs under.

/// Largest text render, `batchName` or PDF form tag output, unless
/// `RUSTICDOC_MAX_RENDER_BYTES` says otherwise.
pub const DEFAULT_MAX_RENDER_BYTES: usize = 64 * 1024 * 1024;

/// File parts one form may hold, unless `RUSTICDOC_MAX_FORM_PARTS` says
/// otherwise.
pub const DEFAULT_MAX_FORM_PARTS: usize = 1_000;

/// Text parts one form may hold.
pub const MAX_TEXT_PARTS: usize = 2_000;

/// Most bytes of documents one request may produce. Limits on parts and pages
/// do not bound this: a thousand parts that each keep a large stream are a
/// thousand copies of it.
pub const MAX_OUTPUT_BYTES: usize = 512 * 1024 * 1024;

/// Most temporary files one LibreOffice conversion may write, unless
/// `RUSTICDOC_LIBREOFFICE_MAX_DISK` says otherwise. A loader or export filter can
/// write without stopping, and the time limit alone lets that fill a disk.
pub const DEFAULT_LIBREOFFICE_MAX_DISK: u64 = MAX_OUTPUT_BYTES as u64;

/// Longest text of one stamp or watermark, in bytes. It is drawn once per page.
pub const MAX_OVERLAY_TEXT: usize = 64 * 1024;

/// Adds `bytes` to what a request has produced, and refuses once that passes
/// `budget`.
pub fn charge_output(total: &mut usize, bytes: usize, budget: usize) -> crate::Result<()> {
    *total = total.saturating_add(bytes);
    if *total > budget {
        return Err(crate::Error::InvalidRequest(format!(
            "the output would pass the limit of {budget} bytes for one request"
        )));
    }
    Ok(())
}

/// Template tags one PDF form may hold.
pub const MAX_FILL_TAGS: usize = 1_000;
