    #[test]
    fn reader_returns_actual_enum_union_and_sequence_fields() {
        let _guard = crate::state::lock_native_runtime_test();
        let writer = allocate_codec_writer();
        for (tag, ordinal, label, auxiliary, scalar) in [
            (CodecEventTag::EnumCase, 2, &b"Ready"[..], 7, 0),
            (CodecEventTag::UnionStart, 3, &b"Named"[..], 1, 0),
            (CodecEventTag::Payload, 0, &b""[..], 0, 0),
            (CodecEventTag::SequenceStart, 0, &b""[..], 3, 0),
            (CodecEventTag::Element, 0, &b""[..], 0, 0),
            (CodecEventTag::Boolean, 0, &b""[..], 0, 1),
            (CodecEventTag::Element, 1, &b""[..], 0, 0),
            (CodecEventTag::Boolean, 0, &b""[..], 0, 0),
            (CodecEventTag::Element, 2, &b""[..], 0, 0),
            (CodecEventTag::Boolean, 0, &b""[..], 0, 1),
            (CodecEventTag::SequenceEnd, 0, &b""[..], 0, 0),
            (CodecEventTag::UnionEnd, 0, &b""[..], 0, 0),
        ] {
            assert_eq!(
                write_event(
                    writer,
                    tag as u8,
                    ordinal,
                    label.as_ptr(),
                    label.len() as u64,
                    auxiliary,
                    scalar,
                ),
                CodecEventStatus::Ok
            );
        }
        let reader = allocate_codec_reader(writer);
        assert_eq!(
            read(reader),
            Ok(ReadEvent {
                tag: CodecEventTag::EnumCase,
                ordinal: 2,
                label: b"Ready".to_vec(),
                auxiliary: 7,
                scalar: 0,
            })
        );
        assert_eq!(
            read(reader),
            Ok(ReadEvent {
                tag: CodecEventTag::UnionStart,
                ordinal: 3,
                label: b"Named".to_vec(),
                auxiliary: 1,
                scalar: 0,
            })
        );
        assert_eq!(
            read(reader),
            Ok(ReadEvent {
                tag: CodecEventTag::Payload,
                ordinal: 0,
                label: Vec::new(),
                auxiliary: 0,
                scalar: 0,
            })
        );
        assert_eq!(
            read(reader),
            Ok(ReadEvent {
                tag: CodecEventTag::SequenceStart,
                ordinal: 0,
                label: Vec::new(),
                auxiliary: 3,
                scalar: 0,
            })
        );
        for expected in [
            CodecEventTag::Element,
            CodecEventTag::Boolean,
            CodecEventTag::Element,
            CodecEventTag::Boolean,
            CodecEventTag::Element,
            CodecEventTag::Boolean,
            CodecEventTag::SequenceEnd,
            CodecEventTag::UnionEnd,
        ] {
            assert_eq!(read(reader).expect("complete union tape").tag, expected);
        }
        assert_eq!(read(reader), Err(CodecEventStatus::MalformedInput));
    }
    #[test]
    fn reader_reconstructs_owned_managed_payloads_after_collection() {
        let _guard = crate::state::lock_native_runtime_test();
        let writer = allocate_codec_writer();
        let string = allocate_utf8_string_literal(b"Pop");
        let bytes = allocate_immutable_bytes(&[0, 1, 255]);
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
        let reader = allocate_codec_reader(writer);
        let reader_root = {
            let mut runtime = lock_abi_runtime().expect("runtime");
            runtime
                .retain_root(ManagedReference::new(reader))
                .expect("root reader")
        };
        {
            let mut runtime = lock_abi_runtime().expect("runtime");
            runtime.request_collection();
            let mut publication = pop_runtime_interface::RootPublication::new(
                pop_runtime_interface::StackMap::new(
                    pop_runtime_interface::SafePointId::new(77),
                    Vec::new(),
                )
                .expect("stack map"),
                Vec::new(),
            )
            .expect("root publication");
            runtime.safe_point(&mut publication).expect("collection");
        }
        let reader = {
            let mut runtime = lock_abi_runtime().expect("runtime");
            runtime
                .resolve_root(reader_root)
                .expect("relocated reader")
                .raw()
        };
        let string = read(reader).expect("String event").scalar;
        let bytes = read(reader).expect("Bytes event").scalar;
        let runtime = lock_abi_runtime().expect("runtime");
        assert_eq!(
            utf8_string_bytes(&runtime, ManagedReference::new(string)),
            Some(b"Pop".to_vec())
        );
        let mut payload = [0; 3];
        runtime
            .immutable_bytes_read(ManagedReference::new(bytes), 0, &mut payload)
            .expect("Bytes payload");
        assert_eq!(payload, [0, 1, 255]);
        drop(runtime);
        lock_abi_runtime()
            .expect("runtime")
            .release_root(reader_root)
            .expect("release reader");
    }
