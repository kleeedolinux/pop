    #[test]
    fn writer_rejects_unknown_tags_invalid_values_and_capabilities() {
        let _guard = crate::state::lock_native_runtime_test();
        let writer = allocate_codec_writer();
        assert_ne!(writer, 0);
        assert_eq!(
            write_event(writer, 27, 0, std::ptr::null(), 0, 0, 0),
            CodecEventStatus::MalformedInput
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
        assert_eq!(
            write_event(
                u64::MAX,
                CodecEventTag::Boolean as u8,
                0,
                std::ptr::null(),
                0,
                0,
                1,
            ),
            CodecEventStatus::CapabilityFailure
        );
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::RecordEnd as u8,
                0,
                std::ptr::null(),
                0,
                0,
                0,
            ),
            CodecEventStatus::MalformedInput
        );
    }
    #[test]
    fn writer_discards_a_failed_aggregate_before_reuse() {
        let _guard = crate::state::lock_native_runtime_test();
        let writer = allocate_codec_writer();
        assert_ne!(writer, 0);
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::EnumCase as u8,
                0,
                b"Before".as_ptr(),
                6,
                0,
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
            write_event(
                writer,
                CodecEventTag::EnumCase as u8,
                1,
                b"After".as_ptr(),
                5,
                1,
                0,
            ),
            CodecEventStatus::Ok
        );

        let reader = allocate_codec_reader(writer);
        assert_eq!(
            read(reader).expect("first committed value").label,
            b"Before"
        );
        assert_eq!(
            read(reader).expect("second committed value").label,
            b"After"
        );
        assert_eq!(read(reader), Err(CodecEventStatus::MalformedInput));
    }

    #[test]
    fn registered_writer_survives_collection_without_a_stack_publication() {
        let _guard = crate::state::lock_native_runtime_test();
        let writer = allocate_codec_writer();
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::Boolean as u8,
                0,
                std::ptr::null(),
                0,
                0,
                0,
            ),
            CodecEventStatus::Ok
        );
        {
            let mut runtime = lock_abi_runtime().expect("runtime");
            runtime.request_collection();
            let mut publication = pop_runtime_interface::RootPublication::new(
                pop_runtime_interface::StackMap::new(
                    pop_runtime_interface::SafePointId::new(78),
                    Vec::new(),
                )
                .expect("stack map"),
                Vec::new(),
            )
            .expect("root publication");
            runtime.safe_point(&mut publication).expect("collection");
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
            CodecEventStatus::Ok
        );
        let reader = allocate_codec_reader(writer);
        assert_eq!(read(reader).expect("first value").scalar, 0);
        assert_eq!(read(reader).expect("second value").scalar, 1);
    }

    #[test]
    fn writer_copies_managed_string_and_bytes_payloads() {
        let _guard = crate::state::lock_native_runtime_test();
        let writer = allocate_codec_writer();
        let string = allocate_utf8_string_literal(b"Pop");
        let bytes = allocate_immutable_bytes(&[0, 1, 255]);
        assert_ne!(string, 0);
        assert_ne!(bytes, 0);
        assert_eq!(
            write_event(
                writer,
                CodecEventTag::String as u8,
                0,
                std::ptr::null(),
                0,
                0,
                string,
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
                bytes,
            ),
            CodecEventStatus::Ok
        );
        let registered = capabilities().lock().expect("codec capabilities");
        let Some(CodecCapability::Writer { events, .. }) =
            registered.get(&writer).map(|entry| &entry.capability)
        else {
            panic!("writer capability")
        };
        assert_eq!(events[0].scalar, StoredScalar::String(b"Pop".to_vec()));
        assert_eq!(events[1].scalar, StoredScalar::Bytes(vec![0, 1, 255]));
    }
