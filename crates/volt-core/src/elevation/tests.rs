use std::sync::Arc;

use super::fakes::{ElevatorCall, FakeElevator};
use super::*;
use crate::fakes::FakeMemory;
use crate::ports::Memory;

fn request(json: &str) -> Result<RequestEnvelope, ErrorCode> {
    decode_request(json.as_bytes())
}

#[test]
fn every_request_round_trips() {
    for envelope in [
        RequestEnvelope {
            id: 1,
            request: Request::Hello { protocol: 1 },
        },
        RequestEnvelope {
            id: 2,
            request: Request::MemoryPurge {
                areas: MemoryArea::RUN_ORDER.to_vec(),
            },
        },
        RequestEnvelope {
            id: 3,
            request: Request::TweakApply {
                tweak: "turn-off-error-reporting".into(),
            },
        },
        RequestEnvelope {
            id: 4,
            request: Request::TweakRevert {
                tweak: "turn-off-error-reporting".into(),
            },
        },
        RequestEnvelope {
            id: 5,
            request: Request::History {
                after: 7,
                limit: HISTORY_MAX_RECORDS,
            },
        },
        RequestEnvelope {
            id: 6,
            request: Request::Bye {},
        },
        RequestEnvelope {
            id: 7,
            request: Request::CleanScan {
                rule: "windows-temp".into(),
                min_age_days: Some(14),
            },
        },
        RequestEnvelope {
            id: 8,
            request: Request::CleanApply {
                scan: "0123456789abcdef0123456789abcdef".into(),
            },
        },
    ] {
        let bytes = encode_request(&envelope).expect("encodes");
        assert!(bytes.len() <= MAX_REQUEST_BYTES);
        assert_eq!(decode_request(&bytes), Ok(envelope));
    }
}

#[test]
fn the_wire_shape_is_stable() {
    assert_eq!(
        request(r#"{"id":1,"request":{"type":"hello","protocol":1}}"#),
        Ok(RequestEnvelope {
            id: 1,
            request: Request::Hello { protocol: 1 }
        })
    );
    assert_eq!(
        request(r#"{"id":4,"request":{"type":"memory_purge","areas":["StandbyList"]}}"#),
        Ok(RequestEnvelope {
            id: 4,
            request: Request::MemoryPurge {
                areas: vec![MemoryArea::StandbyList]
            }
        })
    );
    assert_eq!(
        request(r#"{"id":5,"request":{"type":"bye"}}"#),
        Ok(RequestEnvelope {
            id: 5,
            request: Request::Bye {}
        })
    );
}

#[test]
fn an_oversize_request_is_refused_before_it_is_parsed() {
    let padding = "a".repeat(MAX_REQUEST_BYTES);
    let json = format!(r#"{{"id":1,"request":{{"type":"bye"}},"x":"{padding}"}}"#);
    assert_eq!(request(&json), Err(ErrorCode::TooLarge));
    // Exactly at the cap is fine.
    let mut at_cap = String::from(r#"{"id":1,"request":{"type":"bye"}}"#);
    while at_cap.len() < MAX_REQUEST_BYTES {
        at_cap.push(' ');
    }
    assert_eq!(at_cap.len(), MAX_REQUEST_BYTES);
    assert!(request(&at_cap).is_ok());
    at_cap.push(' ');
    assert_eq!(request(&at_cap), Err(ErrorCode::TooLarge));
}

#[test]
fn unknown_fields_are_refused_not_ignored() {
    for json in [
        // At the top level, in the request, and as a smuggled argument.
        r#"{"id":1,"request":{"type":"bye"},"extra":1}"#,
        r#"{"id":1,"request":{"type":"bye","path":"C:\\x"}}"#,
        r#"{"id":1,"request":{"type":"hello","protocol":1,"cmd":"calc.exe"}}"#,
        r#"{"id":1,"request":{"type":"memory_purge","areas":[],"force":true}}"#,
    ] {
        assert_eq!(request(json), Err(ErrorCode::Malformed), "{json}");
    }
}

#[test]
fn unknown_request_types_are_refused() {
    for json in [
        r#"{"id":1,"request":{"type":"run","cmd":"calc.exe"}}"#,
        r#"{"id":1,"request":{"type":"HELLO","protocol":1}}"#,
        r#"{"id":1,"request":{"type":"restore_point","tweak_id":"x"}}"#,
        r#"{"id":1,"request":{"type":""}}"#,
    ] {
        assert_eq!(request(json), Err(ErrorCode::UnknownRequest), "{json}");
    }
}

#[test]
fn malformed_requests_are_refused() {
    for bytes in [
        &b""[..],
        b"ping",
        b"[]",
        b"null",
        br#"{"id":1}"#,
        br#"{"request":{"type":"bye"}}"#,
        br#"{"id":-1,"request":{"type":"bye"}}"#,
        br#"{"id":"1","request":{"type":"bye"}}"#,
        br#"{"id":1,"request":"bye"}"#,
        br#"{"id":1,"request":{"type":"hello"}}"#,
        br#"{"id":1,"request":{"type":"memory_purge","areas":["Nope"]}}"#,
        &[0xff, 0xfe, 0x00],
    ] {
        assert_eq!(
            decode_request(bytes),
            Err(ErrorCode::Malformed),
            "{}",
            String::from_utf8_lossy(bytes)
        );
    }
}

#[test]
fn ids_must_be_exactly_one_more_each_time() {
    let mut sequencer = RequestSequencer::new();
    assert_eq!(sequencer.accept(0), Err(ErrorCode::OutOfSequence));
    assert_eq!(sequencer.accept(2), Err(ErrorCode::OutOfSequence));
    assert_eq!(sequencer.accept(1), Ok(()));
    assert_eq!(sequencer.accept(1), Err(ErrorCode::OutOfSequence), "repeat");
    assert_eq!(sequencer.accept(3), Err(ErrorCode::OutOfSequence), "gap");
    assert_eq!(sequencer.accept(2), Ok(()));
    assert_eq!(sequencer.accept(1), Err(ErrorCode::OutOfSequence), "back");
    assert_eq!(sequencer.last(), 2);

    let mut at_the_end = RequestSequencer { last: u64::MAX };
    assert_eq!(at_the_end.accept(0), Err(ErrorCode::OutOfSequence));
    assert_eq!(at_the_end.accept(u64::MAX), Err(ErrorCode::OutOfSequence));
}

#[test]
fn only_framing_and_order_violations_close_the_connection() {
    for code in [
        ErrorCode::TooLarge,
        ErrorCode::Malformed,
        ErrorCode::UnknownRequest,
        ErrorCode::OutOfSequence,
        ErrorCode::ProtocolMismatch,
        ErrorCode::Internal,
    ] {
        assert!(code.closes_connection(), "{code}");
    }
    for code in [
        ErrorCode::HelloRequired,
        ErrorCode::InvalidRequest,
        ErrorCode::RateLimited,
        ErrorCode::UnknownTweak,
        ErrorCode::NotConfirmed,
        ErrorCode::Blocked,
        ErrorCode::NothingToRevert,
        ErrorCode::TweakFailed,
        ErrorCode::Unavailable,
        ErrorCode::RestorePointFailed,
        ErrorCode::UnknownRule,
        ErrorCode::UnknownScan,
        ErrorCode::CleanFailed,
    ] {
        assert!(!code.closes_connection(), "{code}");
    }
}

#[test]
fn a_tweak_request_names_an_id_and_nothing_else() {
    assert_eq!(
        request(r#"{"id":2,"request":{"type":"tweak_apply","tweak":"turn-off-fast-startup"}}"#),
        Ok(RequestEnvelope {
            id: 2,
            request: Request::TweakApply {
                tweak: "turn-off-fast-startup".into()
            }
        })
    );
    assert_eq!(
        request(r#"{"id":3,"request":{"type":"tweak_revert","tweak":"x"}}"#),
        Ok(RequestEnvelope {
            id: 3,
            request: Request::TweakRevert { tweak: "x".into() }
        })
    );
    assert_eq!(
        request(r#"{"id":4,"request":{"type":"history","after":0,"limit":5}}"#),
        Ok(RequestEnvelope {
            id: 4,
            request: Request::History { after: 0, limit: 5 }
        })
    );
    // Nothing that could carry a path, a value, a command or a location.
    for json in [
        r#"{"id":1,"request":{"type":"tweak_apply","tweak":"x","path":"C:\x"}}"#,
        r#"{"id":1,"request":{"type":"tweak_apply","tweak":"x","value":1}}"#,
        r#"{"id":1,"request":{"type":"tweak_apply","tweak":"x","command":"calc.exe"}}"#,
        r#"{"id":1,"request":{"type":"tweak_revert","tweak":"x","key":"HKLM\Software"}}"#,
        r#"{"id":1,"request":{"type":"tweak_apply"}}"#,
        r#"{"id":1,"request":{"type":"tweak_apply","tweak":7}}"#,
        r#"{"id":1,"request":{"type":"history","after":-1,"limit":5}}"#,
        r#"{"id":1,"request":{"type":"history","after":0}}"#,
    ] {
        assert_eq!(request(json), Err(ErrorCode::Malformed), "{json}");
    }
}

#[test]
fn the_fake_elevator_serves_the_tweak_requests_through_one_session() {
    let elevator = FakeElevator::granted(Arc::new(FakeMemory::new()));
    let applied = elevator.tweak_apply("a-tweak").expect("apply");
    assert_eq!(applied.outcome, crate::ApplyOutcome::Applied);
    assert!(elevator.tweak_revert("a-tweak").expect("revert").complete);
    assert!(elevator.history(0, 5).expect("history").records.is_empty());
    assert_eq!(elevator.prompts(), 1);
    assert_eq!(
        elevator.calls(),
        vec![
            ElevatorCall::TweakApply("a-tweak".into()),
            ElevatorCall::TweakRevert("a-tweak".into()),
            ElevatorCall::History(0),
        ]
    );
}

#[test]
fn replies_round_trip_and_stay_bounded() {
    let memory = Arc::new(FakeMemory::new());
    // The worst case: every area fails with a long message.
    for area in MemoryArea::RUN_ORDER {
        memory.fail_purge(area, PortError::Other("x".repeat(150)));
    }
    let elevator = FakeElevator::granted(memory);
    let purge = elevator
        .memory_purge(&MemoryArea::RUN_ORDER)
        .expect("purge answers");
    let reply = Reply {
        id: 7,
        body: ReplyBody::MemoryPurge(purge),
    };
    let bytes = encode_reply(&reply);
    assert!(bytes.len() <= MAX_REPLY_BYTES, "{} bytes", bytes.len());
    assert_eq!(decode_reply(&bytes), Ok(reply));

    for body in [
        ReplyBody::Hello(HelloReply {
            protocol: PROTOCOL_VERSION,
            helper_version: "0.1.0".into(),
            catalogues: CatalogueHashes {
                tweaks: catalogue_hash("a"),
                cleaning: catalogue_hash("b"),
            },
        }),
        ReplyBody::Bye {},
        ReplyBody::Error {
            code: ErrorCode::RateLimited,
        },
    ] {
        let reply = Reply { id: 1, body };
        assert_eq!(decode_reply(&encode_reply(&reply)), Ok(reply));
    }
}

#[test]
fn a_reply_too_big_to_send_becomes_an_internal_error() {
    let reply = Reply {
        id: 9,
        body: ReplyBody::Hello(HelloReply {
            protocol: 1,
            helper_version: "v".repeat(MAX_REPLY_BYTES),
            catalogues: CatalogueHashes {
                tweaks: String::new(),
                cleaning: String::new(),
            },
        }),
    };
    assert_eq!(
        decode_reply(&encode_reply(&reply)),
        Ok(Reply {
            id: 9,
            body: ReplyBody::Error {
                code: ErrorCode::Internal
            }
        })
    );
}

#[test]
fn the_catalogue_hash_is_sha256_hex() {
    assert_eq!(
        catalogue_hash(""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_ne!(catalogue_hash("a"), catalogue_hash("b"));
    assert_eq!(catalogue_hash("a").len(), 64);
}

#[test]
fn the_fake_prompts_once_per_session() {
    let memory = Arc::new(FakeMemory::new());
    let elevator = FakeElevator::granted(Arc::clone(&memory) as Arc<dyn Memory>);
    assert_eq!(elevator.prompts(), 0);

    let first = elevator
        .memory_purge(&[MemoryArea::StandbyList])
        .expect("purges");
    assert!(first.any_purged());
    assert!(first.after.expect("after").free_bytes > first.before.expect("before").free_bytes);
    elevator
        .memory_purge(&[MemoryArea::ModifiedList])
        .expect("purges");
    assert_eq!(elevator.prompts(), 1, "the second purge shows no prompt");
    assert_eq!(
        memory.purged(),
        vec![MemoryArea::StandbyList, MemoryArea::ModifiedList]
    );

    elevator.bye();
    assert!(!elevator.is_active());
    elevator.hello().expect("hello");
    assert_eq!(elevator.prompts(), 2, "a new session prompts again");
    assert_eq!(
        elevator.calls(),
        vec![
            ElevatorCall::MemoryPurge(vec![MemoryArea::StandbyList]),
            ElevatorCall::MemoryPurge(vec![MemoryArea::ModifiedList]),
            ElevatorCall::Bye,
            ElevatorCall::Hello,
        ]
    );
}

#[test]
fn a_declined_or_unavailable_elevator_changes_nothing() {
    let memory = Arc::new(FakeMemory::new());
    let declined = FakeElevator::declined(Arc::clone(&memory) as Arc<dyn Memory>);
    assert_eq!(
        declined.memory_purge(&[MemoryArea::StandbyList]),
        Err(ElevationError::Declined)
    );
    assert_eq!(declined.prompts(), 1);

    let unavailable =
        FakeElevator::unavailable(Arc::clone(&memory) as Arc<dyn Memory>, "per-user install");
    assert_eq!(
        unavailable.hello(),
        Err(ElevationError::Unavailable("per-user install".into()))
    );
    assert_eq!(unavailable.prompts(), 0, "no prompt is ever started");
    assert!(memory.purged().is_empty());
}

#[test]
fn the_fake_reports_what_hello_would() {
    let elevator = FakeElevator::granted(Arc::new(FakeMemory::new()));
    elevator.set_catalogue_hashes(CatalogueHashes {
        tweaks: "t".into(),
        cleaning: "c".into(),
    });
    elevator.set_protocol(9);
    let hello = elevator.hello().expect("hello");
    assert_eq!(hello.protocol, 9);
    assert_eq!(hello.catalogues.tweaks, "t");
    assert_eq!(
        elevator.memory_purge(&[]),
        Err(ElevationError::Refused(ErrorCode::InvalidRequest))
    );
}

#[test]
fn the_fake_reports_a_status_without_starting_a_session() {
    let memory = Arc::new(FakeMemory::new());
    let granted = FakeElevator::granted(Arc::clone(&memory) as Arc<dyn Memory>);
    assert_eq!(granted.status(), ElevationStatus::Available);
    assert_eq!(granted.prompts(), 0, "reading the status never prompts");
    assert!(!granted.is_active());

    let standard = FakeElevator::unavailable_because(
        Arc::clone(&memory) as Arc<dyn Memory>,
        UnavailableReason::StandardUser,
    );
    assert_eq!(
        standard.status(),
        ElevationStatus::Unavailable {
            reason: UnavailableReason::StandardUser
        }
    );
    assert_eq!(standard.calls(), vec![ElevatorCall::Status]);
}

#[test]
fn every_unavailable_reason_has_words_and_a_wire_form() {
    for reason in [
        UnavailableReason::NotAllUsersInstall,
        UnavailableReason::HelperMissing,
        UnavailableReason::InstallNotProtected { detail: "x".into() },
        UnavailableReason::StandardUser,
        UnavailableReason::Unknown { detail: "y".into() },
    ] {
        assert!(!reason.to_string().is_empty());
        let json = serde_json::to_string(&reason).expect("encodes");
        assert_eq!(
            serde_json::from_str::<UnavailableReason>(&json).expect("decodes"),
            reason
        );
    }
}

#[test]
fn a_purge_reply_becomes_the_report_an_activity_record_is_written_from() {
    use crate::error::PortError;
    use crate::memory::MemoryCleanReport;

    let reply = MemoryPurgeReply {
        before: None,
        after: None,
        areas: vec![
            AreaResult {
                area: MemoryArea::WorkingSets,
                status: AreaStatus::Purged {
                    processes_trimmed: 5,
                    processes_skipped: 2,
                },
            },
            AreaResult {
                area: MemoryArea::StandbyList,
                status: AreaStatus::Failed {
                    error: PortError::AccessDenied("x".into()),
                },
            },
        ],
    };
    let report = MemoryCleanReport::from(&reply);
    assert!(!report.activity_write_failed);
    assert!(report.any_purged());
    let first = report.areas[0].result.as_ref().expect("purged");
    assert_eq!((first.processes_trimmed, first.processes_skipped), (5, 2));
    assert!(report.areas[1].result.is_err());
}

#[test]
fn a_cleaning_request_names_a_rule_or_a_scan_and_nothing_that_could_carry_a_path() {
    assert_eq!(
        request(r#"{"id":2,"request":{"type":"clean_scan","rule":"windows-temp"}}"#),
        Ok(RequestEnvelope {
            id: 2,
            request: Request::CleanScan {
                rule: "windows-temp".into(),
                min_age_days: None,
            },
        })
    );
    assert_eq!(
        request(
            r#"{"id":3,"request":{"type":"clean_apply","scan":"00112233445566778899aabbccddeeff"}}"#
        ),
        Ok(RequestEnvelope {
            id: 3,
            request: Request::CleanApply {
                scan: "00112233445566778899aabbccddeeff".into(),
            },
        })
    );
    // A path, a pattern or a root is not a field of either request.
    for json in [
        r#"{"id":2,"request":{"type":"clean_scan","rule":"windows-temp","path":"C:\\"}}"#,
        r#"{"id":2,"request":{"type":"clean_scan","rule":"x","root":"C:\Windows"}}"#,
        r#"{"id":3,"request":{"type":"clean_apply","scan":"x","items":["a"]}}"#,
        r#"{"id":3,"request":{"type":"clean_apply"}}"#,
    ] {
        assert_eq!(request(json), Err(ErrorCode::Malformed), "{json}");
    }
}

#[test]
fn the_cleaning_replies_fit_a_reply_with_room_to_spare() {
    use crate::clean::machine::{
        ApplySkips, CleanApplyReply, CleanScanReply, MachineProblem, ScanSkips,
    };
    let scan = Reply {
        id: u64::MAX,
        body: ReplyBody::CleanScan(CleanScanReply {
            scan: "f".repeat(32),
            rule: "a-rule-id-of-some-length-for-the-test".into(),
            files: u64::MAX,
            folders: u64::MAX,
            bytes: u64::MAX,
            min_age_days: u32::MAX,
            truncated: true,
            confirm: true,
            skipped: ScanSkips {
                too_new: u64::MAX,
                age_unknown: u64::MAX,
                links: u64::MAX,
                user_writable: u64::MAX,
                access_denied: u64::MAX,
                unreadable: u64::MAX,
                invalid_name: u64::MAX,
                too_deep: u64::MAX,
                pending_restart: u64::MAX,
            },
            problem: Some(MachineProblem::AccessDenied),
        }),
    };
    let apply = Reply {
        id: u64::MAX,
        body: ReplyBody::CleanApply(CleanApplyReply {
            rule: "a-rule-id-of-some-length-for-the-test".into(),
            files: u64::MAX,
            folders: u64::MAX,
            bytes: u64::MAX,
            skipped: ApplySkips {
                gone: u64::MAX,
                changed: u64::MAX,
                links: u64::MAX,
                hard_links: u64::MAX,
                not_empty: u64::MAX,
                locked: u64::MAX,
                user_writable: u64::MAX,
                pending_restart: u64::MAX,
                failed: u64::MAX,
            },
            stopped: true,
        }),
    };
    for reply in [scan, apply] {
        let bytes = serde_json::to_vec(&reply).expect("json");
        assert!(bytes.len() < MAX_REPLY_BYTES / 2, "{} bytes", bytes.len());
        assert_eq!(decode_reply(&encode_reply(&reply)), Ok(reply));
    }
}

#[test]
fn the_fake_elevator_scans_and_applies_a_machine_rule_in_one_session() {
    let elevator = FakeElevator::granted(Arc::new(FakeMemory::new()));
    let scan = elevator.clean_scan("windows-temp", Some(14)).expect("scan");
    assert_eq!(scan.rule, "windows-temp");
    let done = elevator.clean_apply(&scan.scan).expect("apply");
    assert_eq!(done.rule, "windows-temp");
    assert_eq!(elevator.prompts(), 1);
    assert_eq!(
        elevator.calls()[..2],
        [
            ElevatorCall::CleanScan("windows-temp".into(), Some(14)),
            ElevatorCall::CleanApply(scan.scan.clone()),
        ]
    );
}
