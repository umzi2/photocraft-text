//! Edits the EngineData inside a TySh descriptor without changing its wrapper.

use photocraft_psd::descriptor::{Value as DescValue, VersionedDescriptor};

use crate::engine_data;

use super::Error;

pub(super) fn text_descriptor(block: &[u8]) -> Result<(VersionedDescriptor, usize), Error> {
    let data = block.get(52..).ok_or(Error::TruncatedTySh {
        needed: 52,
        len: block.len(),
    })?;
    Ok(VersionedDescriptor::parse_prefix(data)?)
}

/// Rewrites the `EngineData` blob of a `TySh` block through `edit`.
///
/// The descriptor wrapper (version, transform, text version, warp, bounds) is
/// kept byte for byte; only the `EngineData` value is re-serialised, and the
/// block's even length is preserved.
pub fn with_engine_data(
    block: &[u8],
    edit: impl FnOnce(&mut engine_data::Value),
) -> Result<Vec<u8>, Error> {
    let (mut text_desc, used) = text_descriptor(block)?;
    let version = u16::from_be_bytes([block[0], block[1]]);
    let transform = &block[2..50];
    let text_version = u16::from_be_bytes([block[50], block[51]]);
    // The fixed header has been checked by text_descriptor.
    let warp_at = 52 + used;
    let warp_header = block
        .get(warp_at..warp_at + 2)
        .ok_or(Error::TruncatedTySh {
            needed: warp_at + 2,
            len: block.len(),
        })?;
    let warp_version = u16::from_be_bytes([warp_header[0], warp_header[1]]);
    let (warp_desc, warp_used) = VersionedDescriptor::parse_prefix(&block[warp_at + 2..])?;
    let tail = &block[warp_at + 2 + warp_used..];
    if tail.len() < 16 {
        return Err(Error::TruncatedTySh {
            needed: warp_at + 2 + warp_used + 16,
            len: block.len(),
        });
    }

    let Some(DescValue::RawData(raw)) = text_desc.descriptor.get("EngineData").cloned() else {
        return Err(Error::MissingEngineData);
    };
    let mut ed = engine_data::parse(&raw)?;
    edit(&mut ed);
    let bytes = engine_data::write(&ed);
    for (key, value) in text_desc.descriptor.items.iter_mut() {
        if key.is("EngineData") {
            *value = DescValue::RawData(bytes.clone());
        }
    }

    let mut out = Vec::with_capacity(block.len());
    out.extend_from_slice(&version.to_be_bytes());
    out.extend_from_slice(transform);
    out.extend_from_slice(&text_version.to_be_bytes());
    out.extend_from_slice(&text_desc.to_bytes());
    out.extend_from_slice(&warp_version.to_be_bytes());
    out.extend_from_slice(&warp_desc.to_bytes());
    out.extend_from_slice(tail);
    if out.len() % 2 != 0 {
        out.push(0);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::with_engine_data;
    use crate::engine_data::{self, Value};

    fn tysh(raw: &[u8]) -> Vec<u8> {
        photocraft_psd::TySh::new_text(
            "Hi",
            raw,
            [1.0, 0.0, 0.0, 1.0, 12.5, 40.0],
            [0, 0, 100, 100],
        )
        .to_bytes()
    }

    #[test]
    fn every_truncated_tysh_prefix_returns_an_error() {
        let block = tysh(&engine_data::write(&Value::Dict(Vec::new())));
        for len in 0..block.len() {
            assert!(
                with_engine_data(&block[..len], |_| {}).is_err(),
                "prefix {len}"
            );
        }
    }

    #[test]
    fn no_op_edit_preserves_the_wrapper_bytes() {
        let mut block = tysh(&engine_data::write(&Value::Dict(Vec::new())));
        if block.len() % 2 != 0 {
            block.push(0);
        }
        assert_eq!(with_engine_data(&block, |_| {}).unwrap(), block);
    }

    #[test]
    fn parser_errors_keep_their_sources() {
        let error = with_engine_data(&[0; 52], |_| {}).unwrap_err();
        assert!(std::error::Error::source(&error).is_some());
        let error = with_engine_data(&tysh(b"<< /broken [ >>"), |_| {}).unwrap_err();
        assert!(std::error::Error::source(&error).is_some());
    }
}
