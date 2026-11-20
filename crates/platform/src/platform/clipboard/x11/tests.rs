//! X11 protocol tests on an explicitly selected private display.
use super::*;

#[test]
fn rejects_oversize_before_connecting() {
    let service = X11Clipboard::new(String::new());
    assert_eq!(service.write(&"a".repeat(MAX_CLIPBOARD_BYTES + 1)), Err(NC_E_OUT_OF_RANGE));
    assert!(service.responder.borrow().is_none());
}

/// Requires opt-in to the specific Xvfb instance owned by the fixture runner.
fn display() -> Option<String> {
    if std::env::var_os("NEOCOGI_TEST_ISOLATED_X11").as_deref() != Some("1".as_ref()) {
        return None;
    }
    Some(std::env::var("NEOCOGI_TEST_X11_DISPLAY").expect("pass the isolated Xvfb display explicitly"))
}

/// Gets a real timestamp after opening a peer connection.
fn server_time(peer: &SelectionWindow) -> Timestamp {
    peer.request_timestamp().unwrap();
    loop {
        if let Some(time) = peer.timestamp(&peer.event().unwrap()) {
            return time;
        }
    }
}

/// Claims a test selection at a confirmed X11 barrier.
fn claim(peer: &SelectionWindow) {
    let time = server_time(peer);
    peer.connection.set_selection_owner(peer.id, peer.atoms.CLIPBOARD, time).unwrap();
    assert_eq!(peer.owner(), Ok(peer.id));
}

/// Sends an ordinary SelectionNotify following a checked property write or refusal.
fn reply(peer: &SelectionWindow, event: SelectionRequestEvent, property: Atom) {
    let notification = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: event.time,
        requestor: event.requestor,
        selection: event.selection,
        target: event.target,
        property,
    };
    peer.connection.send_event(false, event.requestor, EventMask::NO_EVENT, notification).unwrap();
}

/// Handles a property-based clipboard request distinct from the production responder.
fn conversion(peer: &SelectionWindow, target: Atom) -> SelectionRequestEvent {
    loop {
        if let Event::SelectionRequest(request) = peer.event().unwrap() {
            assert_eq!(request.target, target);
            return request;
        }
    }
}

/// Waits for a real X11 property read acknowledgement from the other connection.
fn deleted(peer: &SelectionWindow, request: SelectionRequestEvent) {
    loop {
        if let Event::PropertyNotify(event) = peer.event().unwrap()
            && event.window == request.requestor
            && event.atom == request.property
            && event.state == Property::DELETE
        {
            return;
        }
    }
}

/// Determines which wire outcome the independent producer supplies.
enum ExternalText<'a> {
    /// Direct UTF-8 property, possibly invalid bytes.
    Direct(&'a [u8]),
    /// An INCR length and its chunks, which may disagree about the final length.
    Incremental { lower_bound: usize, chunks: &'a [&'a [u8]] },
    /// Advertises a non-text selection.
    NonText,
    /// Claims text support and refuses the conversion.
    Refuse,
}

/// Runs a single peer operation against a minimal producer independent of the service.
fn external_read(display: &str, service: &X11Clipboard, response: ExternalText<'_>) -> Result<String, NcStatus> {
    thread::scope(|scope| {
        let (ready, available) = mpsc::sync_channel(1);
        let worker = scope.spawn(move || {
            let peer = SelectionWindow::new(display, Instant::now() + TRANSFER_TIMEOUT).unwrap();
            claim(&peer);
            ready.send(()).unwrap();

            let request = conversion(&peer, peer.atoms.TARGETS);
            let targets = match response {
                ExternalText::NonText => {
                    let atom = peer.connection.atom(b"image/png").unwrap();
                    [atom]
                }
                _ => [peer.atoms.UTF8_STRING],
            };
            peer.connection
                .change_property32(PropMode::REPLACE, request.requestor, request.property, AtomEnum::ATOM, &targets)
                .unwrap();
            reply(&peer, request, request.property);
            if matches!(response, ExternalText::NonText) {
                return;
            }
            let request = conversion(&peer, peer.atoms.UTF8_STRING);
            match response {
                ExternalText::Direct(value) => {
                    peer.connection
                        .change_property8(PropMode::REPLACE, request.requestor, request.property, peer.atoms.UTF8_STRING, value)
                        .unwrap();
                    reply(&peer, request, request.property);
                }
                ExternalText::Incremental { lower_bound, chunks } => {
                    peer.connection.change_window_attributes(request.requestor, EventMask::PROPERTY_CHANGE).unwrap();
                    peer.connection
                        .change_property32(PropMode::REPLACE, request.requestor, request.property, peer.atoms.INCR, &[lower_bound as u32])
                        .unwrap();
                    reply(&peer, request, request.property);
                    if lower_bound > MAX_CLIPBOARD_BYTES {
                        return;
                    }
                    deleted(&peer, request);
                    let mut sent = 0;
                    for chunk in chunks.iter().copied().chain([&[][..]]) {
                        if peer
                            .connection
                            .change_property8(PropMode::REPLACE, request.requestor, request.property, peer.atoms.UTF8_STRING, chunk)
                            .is_err()
                        {
                            // Oversized read terminates the peer window before the trailing record.
                            return;
                        }
                        sent += chunk.len();
                        if sent > MAX_CLIPBOARD_BYTES {
                            return;
                        }
                        if !chunk.is_empty() {
                            deleted(&peer, request);
                        }
                    }
                }
                ExternalText::Refuse => reply(&peer, request, NONE),
                ExternalText::NonText => unreachable!("only metadata was requested"),
            }
        });
        available.recv_timeout(OPERATION_TIMEOUT).unwrap();
        let result = service.read();
        worker.join().unwrap();
        result
    })
}

#[test]
#[ignore = "requires NEOCOGI_TEST_ISOLATED_X11=1 and NEOCOGI_TEST_X11_DISPLAY on private no-manager Xvfb"]
fn clipboard_validates_external_text_reads_and_keeps_empty_distinct() {
    let Some(display) = display() else { return };
    let service = X11Clipboard::new(display.clone());
    assert_eq!(service.read(), Ok(String::new()), "unowned selection");

    assert_eq!(external_read(&display, &service, ExternalText::Direct(b"")), Ok(String::new()), "empty text");
    assert_eq!(external_read(&display, &service, ExternalText::NonText), Ok(String::new()), "nontext selection");
    assert_eq!(external_read(&display, &service, ExternalText::Direct(&[0xff])), Err(PLATFORM_E_BACKEND));
    assert_eq!(external_read(&display, &service, ExternalText::Refuse), Err(PLATFORM_E_BACKEND));
    assert_eq!(
        external_read(&display, &service, ExternalText::Incremental { lower_bound: 5, chunks: &[b"abc"] }),
        Err(PLATFORM_E_BACKEND),
        "INCR termination below the announced lower bound cannot publish partial text"
    );
    assert_eq!(
        external_read(&display, &service, ExternalText::Incremental { lower_bound: 2, chunks: &[b"abc"] }),
        Ok("abc".to_owned()),
        "the INCR marker is a lower bound rather than a required exact size"
    );

    let payload = "🦀".repeat(MAX_CLIPBOARD_BYTES / 4);
    let chunks: Vec<_> = payload.as_bytes().chunks(MAX_CHUNK_BYTES - 1).collect();
    assert_eq!(
        external_read(
            &display,
            &service,
            ExternalText::Incremental {
                lower_bound: payload.len(),
                chunks: &chunks
            }
        ),
        Ok(payload.clone())
    );
    assert_eq!(
        external_read(
            &display,
            &service,
            ExternalText::Incremental {
                lower_bound: MAX_CLIPBOARD_BYTES + 1,
                chunks: &[]
            }
        ),
        Err(NC_E_OUT_OF_RANGE)
    );
    let mut oversized = chunks.clone();
    oversized.push(b"x");
    assert_eq!(
        external_read(
            &display,
            &service,
            ExternalText::Incremental {
                lower_bound: payload.len(),
                chunks: &oversized
            }
        ),
        Err(NC_E_OUT_OF_RANGE)
    );

    let stalled = SelectionWindow::new(&display, Instant::now() + TRANSFER_TIMEOUT).unwrap();
    claim(&stalled);
    assert_eq!(service.read(), Err(PLATFORM_E_BACKEND), "a real owner who never replies");
    refresh_deadline(&stalled.connection);
    assert_eq!(stalled.owner(), Ok(stalled.id), "a failed read cannot change ownership");
    assert!(service.responder.borrow().is_none(), "read-only clients have no idle worker");
}

#[test]
#[ignore = "requires NEOCOGI_TEST_ISOLATED_X11=1 and NEOCOGI_TEST_X11_DISPLAY on private no-manager Xvfb"]
fn accepted_incremental_write_keeps_its_payload_after_replacement() {
    let Some(display) = display() else { return };
    let selection = X11Clipboard::new(display.clone());
    let original = "🦀".repeat(MAX_CLIPBOARD_BYTES / 4);
    selection.write(&original).unwrap();

    let requester = SelectionWindow::new(&display, Instant::now() + TRANSFER_TIMEOUT).unwrap();
    let target = requester.atoms.UTF8_STRING;
    let reply = requester.convert(target, server_time(&requester), MAX_CLIPBOARD_BYTES).unwrap().unwrap();
    assert_eq!(reply.type_, requester.atoms.INCR);
    assert_eq!(reply.value32().unwrap().collect::<Vec<_>>(), [MAX_CLIPBOARD_BYTES as u32]);

    let replacement = X11Clipboard::new(display.clone());
    replacement.write("new owner é").unwrap();
    assert_eq!(requester.read_text(reply, target), Ok(original));
    assert_eq!(selection.read().as_deref(), Ok("new owner é"), "a read must follow the actual new owner");
    drop(selection);
    assert_eq!(
        replacement.read().as_deref(),
        Ok("new owner é"),
        "retiring old state cannot disown its successor"
    );
    drop(replacement);
    assert_eq!(requester.owner(), Ok(NONE));
}

#[test]
#[ignore = "requires NEOCOGI_TEST_ISOLATED_X11=1 and NEOCOGI_TEST_X11_DISPLAY on private no-manager Xvfb"]
fn cancelled_and_disconnected_writes_cannot_publish_later() {
    let Some(display) = display() else { return };
    let selection = X11Clipboard::new(display.clone());
    selection.write("existing selection").unwrap();
    assert!(matches!(Responder::start(&display, Instant::now()), Err(PLATFORM_E_BACKEND)));
    assert_eq!(
        selection.read().as_deref(),
        Ok("existing selection"),
        "expired setup cannot start a replacement responder"
    );
    let mut responder = selection.responder.borrow_mut();
    let writer = responder.as_mut().unwrap();
    assert!(matches!(writer.publish("cancelled", Instant::now()), Err(PublicationFailure::Unstarted)));
    drop(responder);
    assert_eq!(selection.read().as_deref(), Ok("existing selection"));

    let connection = selection.responder.borrow();
    shutdown(&connection.as_ref().unwrap().socket, Shutdown::Both).unwrap();
    drop(connection);
    assert_eq!(selection.write("worker disconnected"), Err(PLATFORM_E_BACKEND));
    assert!(selection.responder.borrow().is_none());
    assert_eq!(selection.read(), Ok(String::new()));
    selection.write("fresh responder").unwrap();
    assert_eq!(selection.read().as_deref(), Ok("fresh responder"));
}

#[test]
#[ignore = "set NEOCOGI_TEST_ISOLATED_X11=1 to exercise the system libxcb transport"]
fn native_handshake_times_out_without_a_late_connection() {
    if display().is_some() {
        connection::verify_handshake_deadline();
    }
}
