// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::Bytes;
use http::{Request, StatusCode};
use http_body_util::{BodyExt as _, Full};
use rest_over_grpc::serving::serve_http;
use serde_json::{Value, json};

use super::{InstanceToken, Server, advertise};

#[test]
fn startup_prints_a_clean_address_and_shows_the_key_only_for_manual_sign_in() {
    let origin = "http://127.0.0.1:12345";
    let token = InstanceToken("test-key".to_owned());
    let clean = format!("Open {origin}/\n");
    let manual = format!("{clean}Access key: test-key\n");

    let mut output = Vec::new();
    advertise(&mut output, origin, &token, None::<fn(&str) -> std::io::Result<()>>).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), manual);

    let mut output = Vec::new();
    advertise(
        &mut output,
        origin,
        &token,
        Some(|url: &str| {
            assert_eq!(url, format!("{origin}/#test-key"));
            Ok(())
        }),
    )
    .unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), clean);

    let mut output = Vec::new();
    advertise(
        &mut output,
        origin,
        &token,
        Some(|_: &str| Err(std::io::Error::other("launcher failed"))),
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        format!("{clean}Could not open the browser. Open the address above.\nAccess key: test-key\n")
    );
}

async fn request(server: &Server, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Full::new(if body.is_null() {
            Bytes::new()
        } else {
            Bytes::from(body.to_string())
        }))
        .unwrap();
    let response = serve_http(request, &server.rest).await;
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn json_snapshot_round_trip_and_retention() {
    use base64::Engine as _;

    let server = Server::new("127.0.0.1:12345", "test-token");
    let data = base64::engine::general_purpose::STANDARD.encode(crate::mimalloc_view::fixture::container(&[]));
    let open = || {
        request(
            &server,
            "POST",
            "/api/v1/snapshots",
            json!({ "name": "sample.seismograph", "data": data }),
        )
    };
    let (status, snapshot) = open().await;
    assert_eq!(status, StatusCode::OK, "{snapshot}");
    let id = snapshot["id"].as_str().unwrap();
    let (status, page) = request(
        &server,
        "GET",
        &format!("/api/v1/snapshots/{id}/sections/overview?offset=0"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["rows"][0]["title"], "Recorder");

    let threads = format!("/api/v1/snapshots/{id}/sections/threads");
    let (status, page) = request(&server, "GET", &threads, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["rows"][0]["thread"]["id"], "41");
    let (status, filtered) = request(
        &server,
        "PUT",
        &format!("/api/v1/snapshots/{id}/filter"),
        json!({ "include": "crate:unrecorded" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{filtered}");
    assert_eq!(filtered["id"], id);
    let view = filtered["viewId"].as_str().unwrap();
    assert_eq!(
        request(&server, "GET", &format!("{threads}?viewId={view}"), Value::Null).await.0,
        StatusCode::OK
    );
    assert_eq!(request(&server, "GET", &threads, Value::Null).await.1, page);

    let path = format!("/api/v1/snapshots/{id}/file");
    assert_eq!(
        request(&server, "GET", &path, Value::Null).await,
        (StatusCode::OK, json!({ "name": "sample.seismograph", "data": data })),
    );
    let (status, response) = request(&server, "POST", "/api/v1/snapshots", json!({ "data": "YmFk" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(request(&server, "GET", &path, Value::Null).await.0, StatusCode::OK);

    for _ in 0..3 {
        assert_eq!(open().await.0, StatusCode::OK);
        assert_eq!(request(&server, "GET", &path, Value::Null).await.0, StatusCode::OK);
    }
    let (status, latest) = open().await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(request(&server, "GET", &path, Value::Null).await.0, StatusCode::NOT_FOUND);
    let latest = format!("/api/v1/snapshots/{}/file", latest["id"].as_str().unwrap());
    assert_eq!(request(&server, "GET", &latest, Value::Null).await.0, StatusCode::OK);
}
