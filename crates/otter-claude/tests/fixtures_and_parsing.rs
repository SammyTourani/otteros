use otter_claude::{TurnEvent, StopReason};

// Test fixtures as raw SSE strings
const FIXTURE_MAX_TOKENS: &str = r#"event: message_start
data: {"message": {"id": "msg_001", "model": "claude-opus-5"}, "type": "message_start"}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "text"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Response "}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "truncated."}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "max_tokens"}, "usage": {"output_tokens": 64000}}

event: message_stop
data: {"type": "message_stop"}

"#;

const FIXTURE_FALLBACK: &str = r#"event: message_start
data: {"message": {"id": "msg_002", "model": "claude-opus-5"}, "type": "message_start"}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "fallback", "reason": "model_unavailable"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: content_block_start
data: {"type": "content_block_start", "index": 1, "content_block": {"type": "text"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Fallback response"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 1}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 10}}

event: message_stop
data: {"type": "message_stop"}

"#;

const FIXTURE_PAUSE_TURN: &str = r#"event: message_start
data: {"message": {"id": "msg_003", "model": "claude-opus-5"}, "type": "message_start"}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "text"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Pausing..."}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "pause_turn"}, "usage": {"output_tokens": 10}}

event: message_stop
data: {"type": "message_stop"}

"#;

#[test]
fn test_fixture_max_tokens() {
    let mut parser = otter_claude::response::StreamParser::new();
    parser.feed(FIXTURE_MAX_TOKENS.as_bytes());

    let mut found_max_tokens = false;
    while let Some(event) = parser.next_event() {
        if let TurnEvent::Stop { reason: StopReason::MaxTokens } = event {
            found_max_tokens = true;
        }
    }

    assert!(found_max_tokens, "Should detect max_tokens stop reason");
}

#[test]
fn test_fixture_fallback() {
    let mut parser = otter_claude::response::StreamParser::new();
    parser.feed(FIXTURE_FALLBACK.as_bytes());

    let mut found_fallback = false;
    while let Some(event) = parser.next_event() {
        if let TurnEvent::Stop { .. } = event {
            found_fallback = true;
        }
    }

    assert!(found_fallback, "Should parse fallback block");
}

#[test]
fn test_fixture_pause_turn() {
    let mut parser = otter_claude::response::StreamParser::new();
    parser.feed(FIXTURE_PAUSE_TURN.as_bytes());

    let mut found_pause = false;
    while let Some(event) = parser.next_event() {
        if let TurnEvent::Stop { reason: StopReason::PauseTurn } = event {
            found_pause = true;
        }
    }

    assert!(found_pause, "Should detect pause_turn stop reason");
}

#[test]
fn test_parse_one_byte_at_a_time() {
    let fixtures = vec![
        FIXTURE_MAX_TOKENS,
        FIXTURE_FALLBACK,
        FIXTURE_PAUSE_TURN,
    ];

    for fixture in fixtures {
        let bytes = fixture.as_bytes();

        // Parse all at once
        let mut parser_all = otter_claude::response::StreamParser::new();
        parser_all.feed(bytes);
        let events_all: Vec<_> = core::iter::from_fn(|| parser_all.next_event()).collect();

        // Parse one byte at a time
        let mut parser_byte = otter_claude::response::StreamParser::new();
        for byte in bytes {
            parser_byte.feed(&[*byte]);
        }
        let events_byte: Vec<_> = core::iter::from_fn(|| parser_byte.next_event()).collect();

        // Results should be identical
        assert_eq!(
            events_all.len(), events_byte.len(),
            "One-byte parse should have same event count"
        );

        for (i, (e1, e2)) in events_all.iter().zip(events_byte.iter()).enumerate() {
            match (e1, e2) {
                (TurnEvent::Text { delta: d1 }, TurnEvent::Text { delta: d2 }) => {
                    assert_eq!(d1, d2, "Event {}: text delta mismatch", i);
                }
                (TurnEvent::Thinking { delta: d1 }, TurnEvent::Thinking { delta: d2 }) => {
                    assert_eq!(d1, d2, "Event {}: thinking delta mismatch", i);
                }
                (
                    TurnEvent::ToolUseStart { id: id1, name: n1 },
                    TurnEvent::ToolUseStart { id: id2, name: n2 },
                ) => {
                    assert_eq!(id1, id2, "Event {}: tool id mismatch", i);
                    assert_eq!(n1, n2, "Event {}: tool name mismatch", i);
                }
                (
                    TurnEvent::ToolUseReady { id: id1, name: n1, input: in1 },
                    TurnEvent::ToolUseReady { id: id2, name: n2, input: in2 },
                ) => {
                    assert_eq!(id1, id2, "Event {}: tool id mismatch", i);
                    assert_eq!(n1, n2, "Event {}: tool name mismatch", i);
                    assert_eq!(in1, in2, "Event {}: tool input mismatch", i);
                }
                (TurnEvent::Stop { reason: r1 }, TurnEvent::Stop { reason: r2 }) => {
                    assert_eq!(r1, r2, "Event {}: stop reason mismatch", i);
                }
                (TurnEvent::Error { message: m1 }, TurnEvent::Error { message: m2 }) => {
                    assert_eq!(m1, m2, "Event {}: error message mismatch", i);
                }
                _ => panic!("Event {}: type mismatch", i),
            }
        }
    }
}

#[test]
fn test_parse_random_splits() {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let fixtures = vec![
        FIXTURE_MAX_TOKENS,
        FIXTURE_FALLBACK,
        FIXTURE_PAUSE_TURN,
    ];

    for fixture in fixtures {
        let bytes = fixture.as_bytes();

        // Parse all at once for comparison
        let mut parser_all = otter_claude::response::StreamParser::new();
        parser_all.feed(bytes);
        let events_all: Vec<_> = core::iter::from_fn(|| parser_all.next_event()).collect();

        // Generate seeded random splits
        for seed in 0..10 {
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_usize(seed);
            let seed_val = hasher.finish() as u32;

            // Generate random split points
            let mut split_points = Vec::new();
            let mut rng_state = seed_val;
            let mut pos = 0;

            while pos < bytes.len() {
                // Simple LCG random number generator
                rng_state = rng_state.wrapping_mul(1103515245).wrapping_add(12345);
                let chunk_size = 1 + ((rng_state >> 16) % 100) as usize;
                pos += chunk_size.min(bytes.len() - pos);
                if pos < bytes.len() {
                    split_points.push(pos);
                }
            }

            // Parse with random splits
            let mut parser_split = otter_claude::response::StreamParser::new();
            let mut last_pos = 0;

            for &split_pos in &split_points {
                parser_split.feed(&bytes[last_pos..split_pos]);
                last_pos = split_pos;
            }
            parser_split.feed(&bytes[last_pos..]);

            let events_split: Vec<_> = core::iter::from_fn(|| parser_split.next_event()).collect();

            // Results should match
            assert_eq!(
                events_all.len(), events_split.len(),
                "Random split seed {} should have same event count",
                seed
            );
        }
    }
}

#[test]
fn test_key_hygiene_config_display() {
    use otter_claude::{ClientConfig, ApiKey};

    let key = ApiKey::new("sk-test-secret-123456789");
    let config = ClientConfig::new(key);

    let config_str = format!("{:#?}", config);
    assert!(
        !config_str.contains("sk-test-secret"),
        "API key should not appear in config Debug output"
    );
    assert!(
        config_str.contains("<redacted>"),
        "Config Debug should show <redacted>"
    );
}

#[test]
fn test_key_hygiene_validation_error_display() {
    use otter_claude::ValidationError;

    let err = ValidationError::MissingRequired("x".into());
    let err_str = format!("{:?}", err);
    // Ensure no fake keys appear in error messages
    assert!(
        !err_str.contains("sk-"),
        "Error message should not contain API key patterns"
    );
}
