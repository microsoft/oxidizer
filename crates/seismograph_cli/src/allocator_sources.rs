// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Decode allocator sources without conflating their backend-specific state.

use std::sync::Arc;

use seismograph::snapshot::SourceSnapshot;
use seismograph_rallocator::snapshot::{self, BackendState};
use seismograph_rallocator::source;

use crate::allocator_view::Snapshot;
use crate::snapshot_error::Error;

pub(crate) fn decode(sources: &[SourceSnapshot]) -> Result<Snapshot, Error> {
    let native = native_source(sources)?;
    let mimalloc = find(sources, source::MIMALLOC_ID, Error::DuplicateMimallocSource)?;
    let mut view = Snapshot::event_only();
    // Independent backend fields commute; no diagnostic priority is promised.
    // #[gamma::skip(collection.reverse_array)]
    for entry in [native, mimalloc].into_iter().flatten() {
        if entry.schema_version != snapshot::SCHEMA_VERSION {
            return Err(Error::UnsupportedSchema(entry.schema_version));
        }
        let payload = snapshot::decode(&entry.data).map_err(Error::DecodeAllocator)?;
        let (backend, matches_source) = match &payload.backend {
            BackendState::Snmalloc(_) => ("snmalloc", entry.id == source::ID),
            BackendState::Mimalloc(_) => ("mimalloc", entry.id == source::MIMALLOC_ID),
        };
        if !matches_source {
            return Err(Error::SourceBackendMismatch {
                source: if entry.id == source::ID { "snmalloc" } else { "mimalloc" },
                backend,
            });
        }
        attach(&mut view, payload);
    }
    Ok(view)
}

pub(crate) fn native_source(sources: &[SourceSnapshot]) -> Result<Option<&SourceSnapshot>, Error> {
    find(sources, source::ID, Error::DuplicateAllocatorSource)
}

fn find(sources: &[SourceSnapshot], id: seismograph::snapshot::SourceId, duplicate: Error) -> Result<Option<&SourceSnapshot>, Error> {
    let mut matching = sources.iter().filter(|source| source.id == id);
    let source = matching.next();
    if matching.next().is_some() {
        return Err(duplicate);
    }
    Ok(source)
}

fn attach(view: &mut Snapshot, payload: snapshot::Snapshot) {
    let snapshot::Snapshot { header, backend } = payload;
    match backend {
        BackendState::Snmalloc(state) => view.native = Some(Arc::new((header, state).into())),
        BackendState::Mimalloc(state) => {
            view.mimalloc = Some(Arc::new(snapshot::Snapshot {
                header,
                backend: BackendState::Mimalloc(state),
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use seismograph::snapshot::SourceSnapshot;
    use seismograph_rallocator::{ErrorKind, snapshot, source};

    use super::decode;
    use crate::mimalloc_view::fixture;
    use crate::snapshot_error::Error;

    fn decode_container(sources: &[SourceSnapshot]) -> Result<crate::allocator_view::Snapshot, Error> {
        let container = seismograph::snapshot::decode(&fixture::container(sources)).unwrap();
        decode(&container.sources)
    }

    #[test]
    fn an_event_only_container_does_not_invent_backend_evidence() {
        let decoded = decode_container(&[]).unwrap();

        assert_eq!(
            (decoded.native, decoded.mimalloc, decoded.allocator_state_available),
            (None, None, false)
        );
    }

    #[test]
    fn current_snmalloc_payload_is_not_coerced_into_mimalloc_state() {
        let native = crate::native_view::fixture::snapshot();
        let encoded = fixture::source(&snapshot::Snapshot::from(native.clone()));

        let decoded = decode_container(&[encoded]).unwrap();

        assert_eq!((decoded.native.as_deref(), decoded.mimalloc), (Some(&native), None));
    }

    #[test]
    fn mimalloc_tag_under_snmalloc_source_is_rejected() {
        let mut encoded = fixture::source(&fixture::snapshot());
        encoded.id = source::ID;

        let error = decode_container(&[encoded]).unwrap_err();

        assert!(matches!(
            error,
            Error::SourceBackendMismatch {
                source: "snmalloc",
                backend: "mimalloc"
            }
        ));
    }

    #[test]
    fn snmalloc_tag_under_mimalloc_source_is_rejected() {
        let mut encoded = fixture::source(&snapshot::Snapshot::from(crate::native_view::fixture::snapshot()));
        encoded.id = source::MIMALLOC_ID;

        let error = decode_container(&[encoded]).unwrap_err();

        assert!(matches!(
            error,
            Error::SourceBackendMismatch {
                source: "mimalloc",
                backend: "snmalloc"
            }
        ));
    }

    #[test]
    fn previous_outer_schema_is_rejected() {
        let mut encoded = fixture::source(&snapshot::Snapshot::from(crate::native_view::fixture::snapshot()));
        encoded.schema_version = 3;

        let error = decode_container(&[encoded]).unwrap_err();

        assert!(matches!(error, Error::UnsupportedSchema(3)));
    }

    #[test]
    fn current_outer_header_cannot_describe_a_previous_payload() {
        let native = crate::native_view::fixture::snapshot();
        let mut data = vec![0; seismograph_rallocator::encoded_len(&native).unwrap()];
        seismograph_rallocator::encode(&native, &mut data).unwrap();
        data[8..10].copy_from_slice(&3_u16.to_le_bytes());
        let encoded = SourceSnapshot {
            id: source::ID,
            name: "snmalloc".into(),
            schema_version: snapshot::SCHEMA_VERSION,
            data,
        };

        let error = decode_container(&[encoded]).unwrap_err();

        assert!(matches!(error, Error::DecodeAllocator(error) if error.kind() == ErrorKind::UnsupportedSchema(3)));
    }

    #[test]
    fn mimalloc_requires_the_backend_aware_outer_schema() {
        let mut encoded = fixture::source(&fixture::snapshot());
        encoded.schema_version = 3;

        let error = decode_container(&[encoded]).unwrap_err();

        assert!(matches!(error, Error::UnsupportedSchema(3)));
    }

    #[test]
    fn unknown_outer_schema_is_reported_before_decoding_payload_bytes() {
        let mut encoded = fixture::source(&fixture::snapshot());
        encoded.schema_version = 99;
        encoded.data.clear();

        let error = decode_container(&[encoded]).unwrap_err();

        assert_eq!(error.to_string(), "unsupported allocator source schema 99");
    }

    #[test]
    fn malformed_mimalloc_payload_keeps_the_shared_codec_error() {
        let mut encoded = fixture::source(&fixture::snapshot());
        encoded.data.pop();

        let error = decode_container(&[encoded]).unwrap_err();

        assert!(matches!(error, Error::DecodeAllocator(_)));
    }

    #[test]
    fn unknown_backend_tag_is_not_treated_as_snmalloc_or_mimalloc() {
        let mut encoded = fixture::source(&fixture::snapshot());
        encoded.data[10] = u8::MAX;

        let error = decode_container(&[encoded]).unwrap_err();

        assert!(matches!(error, Error::DecodeAllocator(error) if error.kind() == ErrorKind::UnsupportedBackend(u8::MAX)));
    }

    #[test]
    fn duplicate_mimalloc_ids_are_rejected_at_both_container_and_preparation_boundaries() {
        let encoded = fixture::source(&fixture::snapshot());
        let sources = [encoded.clone(), encoded];

        let container_failed = seismograph::snapshot::decode(&fixture::container(&sources)).is_err();
        let preparation = decode(&sources).unwrap_err();

        assert!(container_failed && matches!(preparation, Error::DuplicateMimallocSource));
    }

    #[test]
    fn duplicate_snmalloc_ids_are_rejected_at_both_container_and_preparation_boundaries() {
        let encoded = fixture::source(&snapshot::Snapshot::from(crate::native_view::fixture::snapshot()));
        let sources = [encoded.clone(), encoded];

        let container_failed = seismograph::snapshot::decode(&fixture::container(&sources)).is_err();
        let preparation = decode(&sources).unwrap_err();

        assert!(container_failed && matches!(preparation, Error::DuplicateAllocatorSource));
    }

    #[test]
    fn mimalloc_only_capture_preserves_its_recording_session() {
        let payload = fixture::snapshot();
        let source = fixture::source(&payload);

        let decoded = decode_container(&[source]).unwrap();

        assert_eq!(decoded.mimalloc.as_deref(), Some(&payload));
    }

    #[test]
    fn matching_backend_sessions_remain_in_both_source_payloads() {
        let native = crate::native_view::fixture::snapshot();
        let mut mimalloc = fixture::snapshot();
        mimalloc.header.session_id = native.session_id;
        let sources = [
            fixture::source(&snapshot::Snapshot::from(native.clone())),
            fixture::source(&mimalloc),
        ];

        let decoded = decode_container(&sources).unwrap();

        assert_eq!(
            (decoded.native.as_deref(), decoded.mimalloc.as_deref()),
            (Some(&native), Some(&mimalloc))
        );
    }

    #[test]
    fn independent_backend_sessions_remain_in_their_own_source_payloads() {
        let mut mimalloc = fixture::snapshot();
        mimalloc.header.session_id = 8;
        let sources = [
            fixture::source(&snapshot::Snapshot::from(crate::native_view::fixture::snapshot())),
            fixture::source(&mimalloc),
        ];

        let decoded = decode_container(&sources).unwrap();

        assert_eq!(
            (
                decoded.native.as_deref().unwrap().session_id,
                decoded.mimalloc.as_deref().unwrap().header.session_id
            ),
            (7, 8)
        );
    }
}
