use super::CborError;
use crate::ir_stream::DecodeLimits;

/// Allocation-free structural scan of one definite-length CBOR item.
pub(super) fn check(bytes: &[u8], limits: &DecodeLimits) -> Result<(), CborError> {
    let byte_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if byte_len > limits.max_message_bytes {
        return Err(CborError::MessageTooLarge {
            got: byte_len,
            max: limits.max_message_bytes,
        });
    }
    if byte_len > limits.max_payload_bytes {
        return Err(CborError::LimitExceeded {
            what: "payload bytes",
            got: byte_len,
            max: limits.max_payload_bytes,
        });
    }

    let max_depth = usize::try_from(limits.max_section_count)
        .unwrap_or(usize::MAX)
        .clamp(1, 128);
    let mut offset = 0usize;
    let mut remaining = [0u64; 128];
    remaining[0] = 1;
    let mut depth = 1usize;

    while depth > 0 {
        if remaining[depth - 1] == 0 {
            depth -= 1;
            continue;
        }
        remaining[depth - 1] -= 1;

        let initial = *bytes
            .get(offset)
            .ok_or_else(|| CborError::InvalidCbor("truncated item".to_owned()))?;
        offset += 1;
        let major = initial >> 5;
        let additional = initial & 0x1f;
        if additional == 31 {
            return Err(CborError::InvalidCbor(
                "indefinite-length items are not accepted".to_owned(),
            ));
        }
        let argument = read_argument(bytes, &mut offset, additional)?;

        match major {
            0 | 1 => {}
            2 | 3 => {
                if argument > limits.max_section_bytes {
                    return Err(CborError::LimitExceeded {
                        what: "byte/text string bytes",
                        got: argument,
                        max: limits.max_section_bytes,
                    });
                }
                let length = usize::try_from(argument).map_err(|_| {
                    CborError::InvalidCbor("string length does not fit this target".to_owned())
                })?;
                offset = offset
                    .checked_add(length)
                    .ok_or_else(|| CborError::InvalidCbor("string length overflow".to_owned()))?;
                if offset > bytes.len() {
                    return Err(CborError::InvalidCbor("truncated string".to_owned()));
                }
            }
            4 => push_container(
                &mut remaining,
                &mut depth,
                argument,
                limits.max_elements_per_section,
                max_depth,
                "array elements",
            )?,
            5 => {
                if argument > limits.max_elements_per_section {
                    return Err(CborError::LimitExceeded {
                        what: "map entries",
                        got: argument,
                        max: limits.max_elements_per_section,
                    });
                }
                let items = argument
                    .checked_mul(2)
                    .ok_or_else(|| CborError::InvalidCbor("map item count overflow".to_owned()))?;
                push_container(
                    &mut remaining,
                    &mut depth,
                    items,
                    limits.max_elements_per_section.saturating_mul(2),
                    max_depth,
                    "map items",
                )?;
            }
            6 => push_container(&mut remaining, &mut depth, 1, 1, max_depth, "tagged items")?,
            7 if additional <= 27 => {}
            7 => {
                return Err(CborError::InvalidCbor(
                    "reserved simple-value encoding".to_owned(),
                ));
            }
            _ => unreachable!("CBOR major type is three bits"),
        }
    }

    if offset != bytes.len() {
        return Err(CborError::InvalidCbor(
            "trailing bytes after top-level item".to_owned(),
        ));
    }
    Ok(())
}

fn push_container(
    remaining: &mut [u64; 128],
    depth: &mut usize,
    items: u64,
    max_items: u64,
    max_depth: usize,
    what: &'static str,
) -> Result<(), CborError> {
    if items > max_items {
        return Err(CborError::LimitExceeded {
            what,
            got: items,
            max: max_items,
        });
    }
    if items == 0 {
        return Ok(());
    }
    if *depth >= max_depth {
        return Err(CborError::LimitExceeded {
            what: "nesting depth",
            got: u64::try_from(*depth + 1).unwrap_or(u64::MAX),
            max: u64::try_from(max_depth).unwrap_or(u64::MAX),
        });
    }
    remaining[*depth] = items;
    *depth += 1;
    Ok(())
}

fn read_argument(bytes: &[u8], offset: &mut usize, additional: u8) -> Result<u64, CborError> {
    let width = match additional {
        0..=23 => return Ok(u64::from(additional)),
        24 => 1,
        25 => 2,
        26 => 4,
        27 => 8,
        28..=30 => {
            return Err(CborError::InvalidCbor(
                "reserved additional information".to_owned(),
            ));
        }
        31 => unreachable!("indefinite length rejected by caller"),
        _ => unreachable!("additional information is five bits"),
    };
    let end = offset
        .checked_add(width)
        .ok_or_else(|| CborError::InvalidCbor("argument length overflow".to_owned()))?;
    let encoded = bytes
        .get(*offset..end)
        .ok_or_else(|| CborError::InvalidCbor("truncated argument".to_owned()))?;
    *offset = end;

    let mut value = 0u64;
    for byte in encoded {
        value = (value << 8) | u64::from(*byte);
    }
    Ok(value)
}
