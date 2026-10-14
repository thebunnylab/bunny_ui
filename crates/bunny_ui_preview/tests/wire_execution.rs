use bunny_ui_preview::wire::{
    self, ExecutionMode, Message, Reader, Received, SceneInstanceId, ToHost,
};

fn message(mode: ExecutionMode) -> Message {
    Message::ToHost(ToHost::SetExecution {
        instance: SceneInstanceId::parse("artboard:1").unwrap(),
        variant: 7,
        mode,
    })
}

#[test]
fn execution_is_an_additive_message_with_an_explicit_capability() {
    assert_eq!(wire::WIRE_VERSION, 1);
    assert_eq!(wire::caps::EXECUTION, 16);
    for mode in [ExecutionMode::Design, ExecutionMode::Play] {
        let message = message(mode);
        let bytes = wire::encode(&message);
        assert_eq!(bytes[4], 82);
        let mut reader = Reader::new();
        for byte in bytes {
            reader.feed(&[byte]);
        }
        assert_eq!(reader.pull().unwrap(), Some(Received::Message(message)));
    }
}

#[test]
fn execution_refuses_unknown_modes_truncated_payloads_and_invalid_identities() {
    let bytes = wire::encode(&message(ExecutionMode::Design));
    let mut invalid_mode = bytes.clone();
    *invalid_mode.last_mut().unwrap() = 2;
    let mut invalid_id = bytes.clone();
    invalid_id[9] = b' ';
    let mut truncated = bytes;
    truncated.pop();
    let length = (truncated.len() - 4) as u32;
    truncated[..4].copy_from_slice(&length.to_le_bytes());
    for bytes in [invalid_mode, invalid_id, truncated] {
        let mut reader = Reader::new();
        reader.feed(&bytes);
        assert!(reader.pull().is_err());
    }
}
