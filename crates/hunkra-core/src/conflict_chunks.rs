//! Byte ranges of existing Git conflict markers, separate from explicit index resolution.
use crate::{
    CoreError,
    diff::{MAX_BYTES, MAX_LINES},
};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictChunk {
    pub range: Range<usize>,
    pub a: Range<usize>,
    pub base: Option<Range<usize>>,
    pub b: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkChoice {
    A,
    B,
    AThenB,
    BThenA,
}

/// Recognizes one exact marker line (labels allowed only on opening/base/closing lines).
fn marker(line: &[u8]) -> Option<(u8, usize)> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let first = *line.first()?;
    if !matches!(first, b'<' | b'|' | b'=' | b'>') {
        return None;
    }
    let count = line.iter().take_while(|byte| **byte == first).count();
    if !(7..=512).contains(&count) {
        return None;
    }
    let suffix = &line[count..];
    if !suffix.is_empty() && (first == b'=' || suffix.first() != Some(&b' ')) {
        return None;
    }
    Some((first, count))
}

/// Parses balanced merge/diff3/zdiff3 blocks; malformed/nested marker-like lines disable chunk choices.
pub fn chunks(bytes: &[u8]) -> Result<Vec<ConflictChunk>, CoreError> {
    if bytes.len() > MAX_BYTES || bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
        return Err(CoreError::ConflictUnsupported);
    }
    let mut result = Vec::new();
    let mut open: Option<(usize, usize, usize)> = None;
    let mut base_start = None;
    let mut divider = None;
    let mut offset = 0;
    for (number, line) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
        if number >= MAX_LINES || line.len() > 16_384 {
            return Err(CoreError::ConflictUnsupported);
        }
        let end = offset + line.len();
        if let Some((kind, size)) = marker(line) {
            match (open, kind) {
                (None, b'<') => {
                    open = Some((offset, end, size));
                    base_start = None;
                    divider = None;
                }
                (Some((_, _, width)), b'|')
                    if size == width && base_start.is_none() && divider.is_none() =>
                {
                    base_start = Some((offset, end));
                }
                (Some((_, _, width)), b'=') if size == width && divider.is_none() => {
                    divider = Some((offset, end));
                }
                (Some((start, a_start, width)), b'>') if size == width && divider.is_some() => {
                    let (separator, b_start) = divider.unwrap();
                    result.push(ConflictChunk {
                        range: start..end,
                        a: a_start..base_start.map_or(separator, |(start, _)| start),
                        base: base_start.map(|(_, start)| start..separator),
                        b: b_start..offset,
                    });
                    open = None;
                    base_start = None;
                    divider = None;
                }
                _ => return Err(CoreError::ConflictChunk),
            }
        }
        offset = end;
    }
    if open.is_some() {
        return Err(CoreError::ConflictChunk);
    }
    Ok(result)
}

/// Chooses only the indexed current block, preserving every byte outside its marker range.
pub fn choose_chunk(bytes: &[u8], index: usize, choice: ChunkChoice) -> Result<Vec<u8>, CoreError> {
    choose(bytes, index, choice, None)
}

/// Removes Git's display-only terminal newline only when the actual source proves it was absent.
fn source_ending<'a>(part: &'a [u8], source: &[u8], terminal: bool) -> &'a [u8] {
    if terminal && !source.is_empty() && !source.ends_with(b"\n") {
        for ending in [b"\n".as_slice(), b"\r\n".as_slice()] {
            if let Some(without) = part.strip_suffix(ending)
                && source.ends_with(without)
            {
                return without;
            }
        }
    }
    part
}

/// Uses actual checkout side bytes to retain no-final-newline semantics in the last Git block.
pub(crate) fn choose_with_sides(
    bytes: &[u8],
    index: usize,
    choice: ChunkChoice,
    a: &[u8],
    b: &[u8],
) -> Result<Vec<u8>, CoreError> {
    choose(bytes, index, choice, Some((a, b)))
}

/// Applies one choice with optional verified source EOF information and no implicit separators.
fn choose(
    bytes: &[u8],
    index: usize,
    choice: ChunkChoice,
    sources: Option<(&[u8], &[u8])>,
) -> Result<Vec<u8>, CoreError> {
    let blocks = chunks(bytes)?;
    let chunk = blocks.get(index).ok_or(CoreError::ConflictChunk)?;
    let mut a = &bytes[chunk.a.clone()];
    let mut b = &bytes[chunk.b.clone()];
    if let Some((source_a, source_b)) = sources {
        let terminal = chunk.range.end == bytes.len();
        a = source_ending(a, source_a, terminal);
        b = source_ending(b, source_b, terminal);
    }
    let parts = match choice {
        ChunkChoice::A => [a, b"".as_slice()],
        ChunkChoice::B => [b, b"".as_slice()],
        ChunkChoice::AThenB => [a, b],
        ChunkChoice::BThenA => [b, a],
    };
    if !parts[0].is_empty() && !parts[1].is_empty() && !parts[0].ends_with(b"\n") {
        return Err(CoreError::ConflictBoundary);
    }
    let mut result = bytes[..chunk.range.start].to_vec();
    result.extend(parts[0]);
    result.extend(parts[1]);
    result.extend(&bytes[chunk.range.end..]);
    if result.len() > MAX_BYTES {
        return Err(CoreError::ConflictUnsupported);
    }
    Ok(result)
}
