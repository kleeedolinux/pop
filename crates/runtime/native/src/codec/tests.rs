#[cfg(test)]
mod tests {
    use super::*;
    use crate::{allocate_immutable_bytes, allocate_utf8_string_literal};

    #[derive(Debug, Eq, PartialEq)]
    struct ReadEvent {
        tag: CodecEventTag,
        ordinal: u32,
        label: Vec<u8>,
        auxiliary: u64,
        scalar: u64,
    }

    #[allow(unsafe_code)]
    fn read(reader: u64) -> Result<ReadEvent, CodecEventStatus> {
        let mut tag = 0;
        let mut ordinal = 0;
        let mut label = std::ptr::null();
        let mut label_length = 0;
        let mut auxiliary = 0;
        let mut scalar = 0;
        // SAFETY: all outputs are valid local pointees for this call.
        let status = unsafe {
            pop_rt_codec_read_event(
                reader,
                &mut tag,
                &mut ordinal,
                &mut label,
                &mut label_length,
                &mut auxiliary,
                &mut scalar,
            )
        };
        let Some(status) = CodecEventStatus::from_raw(status) else {
            panic!("closed codec status")
        };
        if status != CodecEventStatus::Ok {
            return Err(status);
        }
        let tag = CodecEventTag::from_raw(tag).expect("runtime returns a closed event tag");
        let label_length = usize::try_from(label_length).expect("bounded label");
        let label = if label_length == 0 {
            Vec::new()
        } else {
            // SAFETY: the successful read returned a borrow valid until the
            // next read; this copies it before that point.
            unsafe { std::slice::from_raw_parts(label, label_length) }.to_vec()
        };
        Ok(ReadEvent {
            tag,
            ordinal,
            label,
            auxiliary,
            scalar,
        })
    }

    include!("tests/writer.rs");
    include!("tests/reader.rs");
    include!("tests/bounds.rs");
}
