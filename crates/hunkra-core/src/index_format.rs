//! Preserves semantic extensions missing from gix-index's current serializer.
use crate::{CoreError, IndexLimitation, repository::git_error};
use std::io::Write;

pub(crate) type UndoStages = [Option<(u32, gix::ObjectId)>; 3];

/// Reads one NUL-terminated REUC field, refusing truncated or excessively long paths/modes.
fn undo_field<'a>(bytes: &mut &'a [u8], limit: usize) -> Result<&'a [u8], CoreError> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(CoreError::UnsupportedWrite)?;
    if end > limit {
        return Err(CoreError::UnsupportedWrite);
    }
    let (field, rest) = bytes.split_at(end);
    *bytes = &rest[1..];
    Ok(field)
}

/// Decodes documented resolve-undo records; official gix currently keeps these fields private.
fn decode_undo(
    mut bytes: &[u8],
    hash: gix::hash::Kind,
) -> Result<std::collections::BTreeMap<Vec<u8>, UndoStages>, CoreError> {
    let mut records = std::collections::BTreeMap::new();
    while !bytes.is_empty() {
        let path = undo_field(&mut bytes, 4096)?.to_vec();
        if path.is_empty() || records.len() >= 100_000 {
            return Err(CoreError::UnsupportedWrite);
        }
        let mut modes = [0; 3];
        for mode in &mut modes {
            let field = undo_field(&mut bytes, 6)?;
            if field.is_empty() || field.iter().any(|byte| !matches!(byte, b'0'..=b'7')) {
                return Err(CoreError::UnsupportedWrite);
            }
            *mode = u32::from_str_radix(std::str::from_utf8(field).unwrap(), 8)
                .map_err(|_| CoreError::UnsupportedWrite)?;
            if !matches!(*mode, 0 | 0o100644 | 0o100755 | 0o120000 | 0o160000) {
                return Err(CoreError::UnsupportedWrite);
            }
        }
        let mut stages = [None; 3];
        for (stage, mode) in stages.iter_mut().zip(modes) {
            if mode != 0 {
                let id = bytes
                    .get(..hash.len_in_bytes())
                    .ok_or(CoreError::UnsupportedWrite)?;
                *stage = Some((mode, gix::ObjectId::from_bytes_or_panic(id)));
                bytes = &bytes[id.len()..];
            }
        }
        if stages.iter().all(Option::is_none) || records.insert(path, stages).is_some() {
            return Err(CoreError::UnsupportedWrite);
        }
    }
    Ok(records)
}

/// Retrieves a previously resolved path without treating old REUC data as a live index conflict.
pub(crate) fn undo_stages(
    bytes: &[u8],
    hash: gix::hash::Kind,
    path: &[u8],
) -> Result<Option<UndoStages>, CoreError> {
    let offset = extension_offset(bytes, hash)?;
    let end = bytes.len() - hash.len_in_bytes();
    let mut found = None;
    for (signature, payload) in gix::index::extension::Iter::new(&bytes[offset..end]) {
        if &signature == b"REUC" {
            if found.is_some() {
                return Err(CoreError::UnsupportedWrite);
            }
            found = Some(decode_undo(payload, hash)?);
        }
    }
    Ok(found.and_then(|mut records| records.remove(path)))
}

/// Replaces one resolve-undo record while retaining every other path's stages and object ids.
pub(crate) fn with_resolve_undo(
    preserved: &[u8],
    hash: gix::hash::Kind,
    path: &[u8],
    stages: UndoStages,
) -> Result<Vec<u8>, CoreError> {
    let mut records = std::collections::BTreeMap::new();
    let mut seen = false;
    for (signature, payload) in gix::index::extension::Iter::new(preserved) {
        if &signature != b"REUC" || seen {
            return Err(CoreError::UnsupportedWrite);
        }
        seen = true;
        records = decode_undo(payload, hash)?;
    }
    records.insert(path.to_vec(), stages);
    let mut payload = Vec::new();
    for (path, stages) in records {
        payload.extend(path);
        payload.push(0);
        for stage in stages {
            payload.extend(format!("{:o}", stage.map_or(0, |(mode, _)| mode)).bytes());
            payload.push(0);
        }
        for (_, id) in stages.into_iter().flatten() {
            payload.extend(id.as_slice());
        }
    }
    let mut extension = b"REUC".to_vec();
    extension.extend(
        u32::try_from(payload.len())
            .map_err(|_| CoreError::UnsupportedWrite)?
            .to_be_bytes(),
    );
    extension.extend(payload);
    Ok(extension)
}

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
