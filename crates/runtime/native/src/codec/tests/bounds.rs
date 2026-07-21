    #[test]
    fn writer_enforces_exact_event_and_payload_bounds() {
        let _guard = crate::state::lock_native_runtime_test();
        let writer = allocate_codec_writer();
        for _ in 0..MAX_CODEC_EVENTS {
            assert_eq!(
                write_event(
                    writer,
                    CodecEventTag::Boolean as u8,
                    0,
                    std::ptr::null(),
                    0,
                    0,
                    1,
                ),
                CodecEventStatus::Ok
            );
        }
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::Boolean as u8,
                0,
                std::ptr::null(),
                0,
                0,
                1,
            ),
            CodecEventStatus::LimitExceeded
        );

        let writer = allocate_codec_writer();
        let accepted = allocate_immutable_bytes(&vec![0; MAX_CODEC_PAYLOAD_BYTES]);
        let rejected = allocate_immutable_bytes(&vec![0; MAX_CODEC_PAYLOAD_BYTES + 1]);
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::Bytes as u8,
                0,
                std::ptr::null(),
                0,
                0,
                accepted,
            ),
            CodecEventStatus::Ok
        );
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::Bytes as u8,
                0,
                std::ptr::null(),
                0,
                0,
                rejected,
            ),
            CodecEventStatus::LimitExceeded
        );

        let writer = allocate_codec_writer();
        let text_bytes = vec![b'x'; MAX_CODEC_PAYLOAD_BYTES + 1];
        let text = allocate_utf8_string_literal(&text_bytes);
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::String as u8,
                0,
                std::ptr::null(),
                0,
                0,
                text,
            ),
            CodecEventStatus::Ok,
            "the 65,535 payload bound applies to Bytes, not String"
        );
    }

    #[test]
    fn reader_never_observes_a_partially_written_top_level_value() {
        let _guard = crate::state::lock_native_runtime_test();
        let writer = allocate_codec_writer();
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::EnumCase as u8,
                0,
                b"Ready".as_ptr(),
                5,
                7,
                0,
            ),
            CodecEventStatus::Ok
        );
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::RecordStart as u8,
                0,
                std::ptr::null(),
                0,
                1,
                0,
            ),
            CodecEventStatus::Ok
        );
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::Member as u8,
                0,
                b"active".as_ptr(),
                6,
                0,
                0,
            ),
            CodecEventStatus::Ok
        );
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::Boolean as u8,
                0,
                std::ptr::null(),
                0,
                0,
                2,
            ),
            CodecEventStatus::MalformedInput
        );

        let reader = allocate_codec_reader(writer);
        assert_eq!(
            read(reader),
            Ok(ReadEvent {
                tag: CodecEventTag::EnumCase,
                ordinal: 0,
                label: b"Ready".to_vec(),
                auxiliary: 7,
                scalar: 0,
            })
        );
        assert_eq!(
            read(reader),
            Err(CodecEventStatus::MalformedInput),
            "an incomplete second value must not be published"
        );

        let writer = allocate_codec_writer();
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::RecordStart as u8,
                0,
                std::ptr::null(),
                0,
                1,
                0,
            ),
            CodecEventStatus::Ok
        );
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::SequenceStart as u8,
                0,
                std::ptr::null(),
                0,
                (MAX_CODEC_PAYLOAD_BYTES + 1) as u64,
                0,
            ),
            CodecEventStatus::LimitExceeded
        );
        assert_eq!(
            read(allocate_codec_reader(writer)),
            Err(CodecEventStatus::MalformedInput),
            "a limit failure must discard the unpublished aggregate"
        );

        let writer = allocate_codec_writer();
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::UnionStart as u8,
                0,
                b"Value".as_ptr(),
                5,
                1,
                0,
            ),
            CodecEventStatus::Ok
        );
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::String as u8,
                0,
                std::ptr::null(),
                0,
                0,
                u64::MAX,
            ),
            CodecEventStatus::CapabilityFailure
        );
        assert_eq!(
            read(allocate_codec_reader(writer)),
            Err(CodecEventStatus::MalformedInput),
            "a capability failure must discard the unpublished aggregate"
        );
    }
