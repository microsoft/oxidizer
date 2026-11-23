// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;
use crate::commands::monitor::snapshot;

#[test]
fn runtime_detail_follows_ids_across_worker_order_and_task_sort() {
    let bytes = crate::mimalloc_view::fixture::container(&[]);
    let mut capture = snapshot::prepare(::seismograph::snapshot::decode(&bytes).unwrap()).unwrap();
    let task = |task_id, future_size_bytes| RuntimeTaskSummary {
        runtime_id: 7,
        task_id,
        future_size_bytes: Some(future_size_bytes),
        spawn_stack: vec!["executor::poll".into(), "fixture::spawn".into()],
        application_spawn_stack: vec!["fixture::spawn".into()],
        ..RuntimeTaskSummary::default()
    };
    capture.runtime.workers = vec![
        RuntimeWorkerSummary {
            runtime_id: 8,
            worker_id: Some(3),
            ..RuntimeWorkerSummary::default()
        },
        RuntimeWorkerSummary {
            runtime_id: 7,
            worker_id: Some(3),
            tasks: vec![task(21, 256), task(42, 16)],
            ..RuntimeWorkerSummary::default()
        },
    ];
    let mut request = api::InspectRequest {
        section: "runtime".into(),
        parent: "r/7/3".into(),
        sort_by: "future".into(),
        descending: true,
        ..api::InspectRequest::default()
    };
    let page = inspect(&capture, &request).unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.rows[0].title, "Task 21");

    request.parent = "r/7/3/42".into();
    request.descending = false;
    capture.runtime.workers.swap(0, 1);
    let detail = inspect(&capture, &request).unwrap();
    let subject = detail.subject.unwrap();
    assert!(matches!(
        subject.model,
        Some(Model::RuntimeTask(api::RuntimeTask {
            task_id: 42,
            future_size_bytes: Some(16),
            ..
        }))
    ));
    assert_eq!(subject.stack, ["fixture::spawn"]);
    assert_eq!(detail.total, 0);

    capture.runtime.workers[0].tasks.retain(|task| task.task_id != 42);
    let err = inspect(&capture, &request).unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
}
