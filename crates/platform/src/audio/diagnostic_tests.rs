//! Actual opening-request COM failures retain normalized text independently of the request.
use super::*;
use neocogi_com::INcStringComPtrExt;

/// Failed direction takes preserve the error; cancellation/drop release the request, not its text snapshot.
#[test]
fn failed_opening_retains_bounded_owned_diagnostics_through_real_com() {
    for cancel in [false, true] {
        let mut storage = String::with_capacity(MAX_AUDIO_ERROR_BYTES * 32);
        storage.push_str(&"é".repeat(MAX_AUDIO_ERROR_BYTES));
        let error = AudioError::new(AUDIO_E_BACKEND, storage);
        assert!(error.message.capacity() <= MAX_AUDIO_ERROR_BYTES);
        let mut request: ComBox<IAudioOpenRequestAbi> = OpenRequest::into_local_box(OpenRequest { state: RequestState::Failed(error) })
            .into_interface()
            .unwrap_or_else(|(status, _)| panic!("request interface: {status}"));
        assert_eq!(request.poll().unwrap(), AudioOpenState::Failed(AUDIO_E_BACKEND.code()));
        let text = request.message().unwrap();
        assert_eq!(request.take_playback().err(), Some(AUDIO_E_BACKEND));
        assert_eq!(request.take_capture().err(), Some(AUDIO_E_BACKEND));
        assert_eq!(request.poll().unwrap(), AudioOpenState::Failed(AUDIO_E_BACKEND.code()));
        if cancel {
            request.cancel().unwrap();
            assert_eq!(request.poll().unwrap(), AudioOpenState::Cancelled);
            assert_eq!(request.message().unwrap().to_string_bounded(MAX_AUDIO_ERROR_BYTES).unwrap(), "");
        }
        drop(request);
        let retained = text.to_string_bounded(MAX_AUDIO_ERROR_BYTES).unwrap();
        assert_eq!(retained, "é".repeat(MAX_AUDIO_ERROR_BYTES / 2));
    }
}
