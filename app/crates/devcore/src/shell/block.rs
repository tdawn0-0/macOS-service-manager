//! Marker-block editing, ported from `runtime_shell.go:updateShellBlock`.
//!
//! A block is identified by a unique begin and end marker, each on its own
//! line. Enabling replaces an existing block in place, preserving the user's
//! surrounding text and ordering; disabling removes only the block. Applying
//! the same enable twice is byte-for-byte idempotent.

use super::ShellError;

/// Replace, insert or remove one marked block.
pub fn update_shell_block(
    data: &[u8],
    begin: &str,
    end: &str,
    block: &str,
    enable: bool,
) -> Result<Vec<u8>, ShellError> {
    let begin = begin.as_bytes();
    let end = end.as_bytes();
    let mut out = data.to_vec();
    let start = find(&out, begin);
    let stop = find(&out, end);
    if out
        .windows(begin.len())
        .filter(|window| *window == begin)
        .count()
        > 1
        || out
            .windows(end.len())
            .filter(|window| *window == end)
            .count()
            > 1
    {
        return Err(ShellError::invalid("duplicate runtime shell blocks"));
    }
    if start.is_some() != stop.is_some()
        || stop.is_some_and(|stop| start.is_some_and(|start| stop < start))
    {
        return Err(ShellError::invalid("damaged runtime shell block"));
    }
    if let (Some(start), Some(stop)) = (start, stop) {
        let mut finish = stop + end.len();
        if (start > 0 && out[start - 1] != b'\n') || (finish < out.len() && out[finish] != b'\n') {
            return Err(ShellError::invalid(
                "runtime shell markers must be on separate lines",
            ));
        }
        if finish < out.len() {
            finish += 1;
        }
        if enable {
            // Upgrade in place, preserving the user's ordering and text.
            let mut next = out[..start].to_vec();
            next.extend_from_slice(block.as_bytes());
            next.extend_from_slice(&out[finish..]);
            return Ok(next);
        }
        let mut start = start;
        // Node removal also trims one blank line this block introduced.
        if begin == super::NODE_BLOCK_BEGIN.as_bytes()
            && start > 0
            && out[start - 1] == b'\n'
            && (start == 1 || out[start - 2] == b'\n')
        {
            start -= 1;
        }
        let mut next = out[..start].to_vec();
        next.extend_from_slice(&out[finish..]);
        return Ok(next);
    }
    if !enable {
        return Ok(out);
    }
    if !out.is_empty() && out[out.len() - 1] != b'\n' {
        out.push(b'\n');
    }
    if begin == super::NODE_BLOCK_BEGIN.as_bytes() && !out.is_empty() {
        out.push(b'\n');
    }
    out.extend_from_slice(block.as_bytes());
    Ok(out)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
