//! Preserves semantic extensions missing from gix-index's current serializer.
use crate::{CoreError, IndexLimitation, repository::git_error};
use std::io::Write;

/// Finds the extension boundary of an already validated v2/v3/v4 index without decoding paths.
fn extension_offset(bytes: &[u8], hash: gix::hash::Kind) -> Result<usize, CoreError> {
    if let Some(offset) =
        gix::index::extension::end_of_index_entry::decode(bytes, hash).map_err(git_error)?
    {
        return Ok(offset);
    }
    let invalid = || CoreError::UnsupportedWrite;
    let version = u32::from_be_bytes(bytes.get(4..8).ok_or_else(invalid)?.try_into().unwrap());
    let count = u32::from_be_bytes(bytes.get(8..12).ok_or_else(invalid)?.try_into().unwrap());
    let end = bytes
        .len()
        .checked_sub(hash.len_in_bytes())
        .ok_or_else(invalid)?;
    let mut offset = 12usize;
    for _ in 0..count {
        let start = offset;
        offset = offset
            .checked_add(40 + hash.len_in_bytes())
            .ok_or_else(invalid)?;
        let flags = u16::from_be_bytes(
            bytes
                .get(offset..offset + 2)
                .ok_or_else(invalid)?
                .try_into()
                .unwrap(),
        );
        offset += 2;
        if flags & 0x4000 != 0 {
            offset += 2;
        }
        if version == 4 {
            // We only skip the OFS_DELTA prefix length; gix has already validated its value.
            loop {
                let byte = *bytes.get(offset).ok_or_else(invalid)?;
                offset += 1;
                if byte & 0x80 == 0 {
                    break;
                }
            }
        }
        let remaining = bytes.get(offset..end).ok_or_else(invalid)?;
        offset += remaining
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(invalid)?
            + 1;
        if version != 4 {
            offset = start + (offset - start).next_multiple_of(8);
        }
        if offset > end {
            return Err(invalid());
        }
    }
    Ok(offset)
}

/// Copies REUC verbatim; performance caches may be invalidated and unknown semantics are refused.
pub(crate) fn preserve(bytes: &[u8], hash: gix::hash::Kind) -> Result<Vec<u8>, CoreError> {
    let offset = extension_offset(bytes, hash)?;
    let end = bytes.len() - hash.len_in_bytes();
    let extensions = bytes.get(offset..end).ok_or(CoreError::UnsupportedWrite)?;
    let mut iter = gix::index::extension::Iter::new(extensions);
    let mut preserved = Vec::new();
    for (signature, payload) in iter.by_ref() {
        match &signature {
            b"REUC" => {
                preserved.extend(signature);
                preserved.extend(
                    u32::try_from(payload.len())
                        .map_err(|_| CoreError::UnsupportedWrite)?
                        .to_be_bytes(),
                );
                preserved.extend(payload);
            }
            b"TREE" | b"UNTR" | b"FSMN" | b"EOIE" | b"IEOT" => {}
            // In particular, a split/sparse index must never be flattened accidentally.
            _ => return Err(CoreError::IndexVariant(IndexLimitation::UnknownExtension)),
        }
    }
    if iter.consumed != extensions.len() {
        return Err(CoreError::UnsupportedWrite);
    }
    Ok(preserved)
}

/// Uses gix's entry serializer and hash writer, adding preserved semantic data before checksum.
pub(crate) fn write(
    index: &gix::index::State,
    preserved: &[u8],
    mut out: impl Write,
) -> Result<(), CoreError> {
    let mut hashed = gix::hash::io::Write::new(&mut out, index.object_hash());
    index
        .write_to(
            &mut hashed,
            gix::index::write::Options {
                extensions: gix::index::write::Extensions::None,
                skip_hash: false,
            },
        )
        .map_err(git_error)?;
    hashed.write_all(preserved)?;
    let checksum = hashed.hash.try_finalize().map_err(git_error)?;
    out.write_all(checksum.as_slice())?;
    Ok(())
}
