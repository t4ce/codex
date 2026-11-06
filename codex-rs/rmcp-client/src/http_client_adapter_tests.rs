use std::io::ErrorKind;

use codex_exec_server::HttpRedirectPolicy;
use http::HeaderMap;
use http::HeaderValue;
use http::header::AUTHORIZATION;
use pretty_assertions::assert_eq;

use super::HttpHeader;
use super::SseEventSizeLimit;
use super::StreamableHttpRedirectMode;
use super::mcp_redirect_policy;
use super::protocol_headers;

#[test]
fn protocol_headers_preserve_utf8_values() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-plugin-name",
        HeaderValue::from_str("café").expect("valid HTTP field value"),
    );

    assert_eq!(
        protocol_headers(&headers),
        vec![HttpHeader {
            name: "x-plugin-name".to_string(),
            value: "café".to_string(),
            value_env_var: None,
        }]
    );
}

#[test]
fn legacy_configured_headers_follow_redirects() {
    assert_eq!(
        mcp_redirect_policy(
            StreamableHttpRedirectMode::Legacy,
            &HeaderMap::new(),
            /*has_configured_headers*/ true,
        ),
        HttpRedirectPolicy::Follow
    );
}

#[test]
fn agent_plugin_configured_headers_stop_redirects() {
    assert_eq!(
        mcp_redirect_policy(
            StreamableHttpRedirectMode::AgentPluginV1,
            &HeaderMap::new(),
            /*has_configured_headers*/ true,
        ),
        HttpRedirectPolicy::Stop
    );
}

#[test]
fn requests_without_sensitive_headers_follow_redirects() {
    assert_eq!(
        mcp_redirect_policy(
            StreamableHttpRedirectMode::AgentPluginV1,
            &HeaderMap::new(),
            /*has_configured_headers*/ false,
        ),
        HttpRedirectPolicy::Follow
    );
}

#[test]
fn authorization_redirects_depend_on_mode() {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer secret"));
    assert_eq!(
        mcp_redirect_policy(
            StreamableHttpRedirectMode::Legacy,
            &headers,
            /*has_configured_headers*/ false,
        ),
        HttpRedirectPolicy::Follow
    );
    assert_eq!(
        mcp_redirect_policy(
            StreamableHttpRedirectMode::AgentPluginV1,
            &headers,
            /*has_configured_headers*/ false,
        ),
        HttpRedirectPolicy::Stop
    );
}

#[test]
fn runtime_assertions_stop_redirects_in_both_mcp_modes() {
    for mode in [
        StreamableHttpRedirectMode::Legacy,
        StreamableHttpRedirectMode::AgentPluginV1,
    ] {
        for (name, value) in [
            ("x-openai-agent-assertion", "1.protected.payload.signature"),
            (
                "authorization",
                "AgentAssertion 1.protected.payload.signature",
            ),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(name, HeaderValue::from_static(value));
            assert_eq!(
                mcp_redirect_policy(mode, &headers, /*has_configured_headers*/ false),
                HttpRedirectPolicy::Stop,
            );
        }
    }
}

#[test]
fn lf_terminators_reset_the_event_limit() {
    let mut limit = SseEventSizeLimit::new(Some(8));

    limit
        .observe(b"data: a\n\ndata: b\n\n")
        .expect("LF-terminated events must have independent size limits");

    assert_eq!((limit.retained_bytes, limit.line_bytes), (0, 0));
}

#[test]
fn carriage_return_terminators_reset_the_event_limit() {
    let mut limit = SseEventSizeLimit::new(Some(8));

    limit
        .observe(b"data: a\r\rdata: b\r\r")
        .expect("CR-terminated events must have independent size limits");

    assert_eq!((limit.retained_bytes, limit.line_bytes), (0, 0));
}

#[test]
fn crlf_terminators_split_across_chunks_reset_the_event_limit() {
    let mut limit = SseEventSizeLimit::new(Some(8));

    limit
        .observe(b"data: a\r")
        .expect("a CR must finish the first event field");
    limit
        .observe(b"\n\r")
        .expect("a split CRLF must not finish another field");
    limit
        .observe(b"\ndata: b\r")
        .expect("the blank CRLF must reset the first event");
    limit
        .observe(b"\n\r\n")
        .expect("the second split CRLF event must remain within the limit");

    assert_eq!((limit.retained_bytes, limit.line_bytes), (0, 0));
}

#[test]
fn event_at_the_exact_size_limit_is_accepted() {
    let mut limit = SseEventSizeLimit::new(Some(9));

    limit
        .observe(b"data: ab\n\n")
        .expect("an event at the exact limit must be accepted");

    assert_eq!((limit.retained_bytes, limit.line_bytes), (0, 0));
}

#[test]
fn completed_keepalive_comments_do_not_accumulate() {
    let mut limit = SseEventSizeLimit::new(Some(6));

    limit
        .observe(&b": ping\n".repeat(/*n*/ 64))
        .expect("completed keepalive comments must not accumulate");

    assert_eq!((limit.retained_bytes, limit.line_bytes), (0, 0));
}

#[test]
fn split_keepalive_comments_are_discarded_only_when_complete() {
    let mut limit = SseEventSizeLimit::new(Some(6));

    limit
        .observe(b": pi")
        .expect("an incomplete comment within the limit must be retained");
    assert_eq!((limit.retained_bytes, limit.line_bytes), (0, 4));

    limit
        .observe(b"ng\n: ping\n")
        .expect("only completed comments may be excluded from the limit");

    assert_eq!((limit.retained_bytes, limit.line_bytes), (0, 0));
}

#[test]
fn comments_do_not_reset_accumulated_event_data() {
    let mut limit = SseEventSizeLimit::new(Some(14));

    limit
        .observe(b"data: a\n")
        .expect("the first data field must fit");
    limit
        .observe(b": ping\n")
        .expect("a completed comment must not count as event data");

    let error = limit
        .observe(b"data: b\n")
        .expect_err("a comment must not reset previously retained data");

    assert_eq!(
        (error.kind(), error.to_string()),
        (
            ErrorKind::InvalidData,
            "MCP response body exceeds 14 bytes".to_string(),
        )
    );
}

#[test]
fn an_unterminated_comment_remains_size_limited() {
    let mut limit = SseEventSizeLimit::new(Some(6));

    limit
        .observe(b": ping")
        .expect("a comment at the exact limit must be accepted");

    let error = limit
        .observe(b"!")
        .expect_err("an unterminated comment must not bypass the limit");

    assert_eq!(
        (error.kind(), error.to_string()),
        (
            ErrorKind::InvalidData,
            "MCP response body exceeds 6 bytes".to_string(),
        )
    );
}

#[test]
fn multiline_data_counts_parser_inserted_newlines() {
    let mut limit = SseEventSizeLimit::new(Some(18));

    let error = limit
        .observe(b"data: aaa\ndata: bbb\n\n")
        .expect_err("joined multiline event data must remain size limited");

    assert_eq!(
        (error.kind(), error.to_string()),
        (
            ErrorKind::InvalidData,
            "MCP response body exceeds 18 bytes".to_string(),
        )
    );
}

#[test]
fn legacy_sse_streams_remain_unlimited() {
    let mut limit = SseEventSizeLimit::new(/*maximum_bytes*/ None);

    limit
        .observe(&[b'x'; 128])
        .expect("legacy SSE streams must not gain a message limit");

    assert_eq!(
        (
            limit.retained_bytes,
            limit.line_bytes,
            limit.line_is_comment,
            limit.previous_was_carriage_return,
            limit.failed,
        ),
        (0, 0, false, false, false)
    );
}

#[test]
fn rejected_events_remain_rejected() {
    let mut limit = SseEventSizeLimit::new(Some(8));

    limit
        .observe(b"data: abc")
        .expect_err("an oversized event must be rejected");

    let error = limit
        .observe(b"")
        .expect_err("a rejected event must not be resumed");

    assert_eq!(
        (error.kind(), error.to_string()),
        (
            ErrorKind::InvalidData,
            "oversized MCP SSE event was already rejected".to_string(),
        )
    );
}

#[test]
fn size_accounting_saturates_without_overflow() {
    let mut limit = SseEventSizeLimit::new(Some(usize::MAX - 1));
    limit.retained_bytes = usize::MAX - 2;
    limit.line_bytes = 1;

    let error = limit
        .observe(b"x")
        .expect_err("saturating counts must still reject an oversized event");

    assert_eq!((error.kind(), limit.failed), (ErrorKind::InvalidData, true));
}

#[tokio::test]
async fn observed_body_preserves_chunks_and_reports_first_byte_and_total() {
    let capture = crate::mcp_phase_timing::Capture::new("http", Some("tools/list"), None).unwrap();
    let mut body = codex_exec_server::HttpResponseBodyStream::from_chunks(vec![
        b"abc".to_vec(),
        b"defg".to_vec(),
    ]);
    let actual = super::collect_body_observed(&mut body, None, Some(&capture))
        .await
        .unwrap();
    assert_eq!(actual, b"abcdefg");
    assert_eq!(
        capture.test_records(),
        vec![
            ("body", "started", 0),
            ("first_body_byte", "complete", 3),
            ("body", "complete", 7)
        ]
    );
}

#[tokio::test]
async fn observed_body_limit_is_reported_as_error_without_changing_limit() {
    let capture = crate::mcp_phase_timing::Capture::new("http", Some("tools/list"), None).unwrap();
    let mut body = codex_exec_server::HttpResponseBodyStream::from_chunks(vec![b"abc".to_vec()]);
    assert!(
        super::collect_body_observed(&mut body, Some(2), Some(&capture))
            .await
            .is_err()
    );
    assert_eq!(capture.test_records().last(), Some(&("body", "error", 3)));
}

#[tokio::test]
async fn observed_sse_preserves_events_and_records_first_parsed_event() {
    use futures::StreamExt;
    let capture = crate::mcp_phase_timing::Capture::new("http", Some("tools/list"), None).unwrap();
    let body = codex_exec_server::HttpResponseBodyStream::from_chunks(vec![
        b"data: one\n\n".to_vec(),
        b"data: two\n\n".to_vec(),
    ]);
    let mut stream = super::sse_stream_from_body_observed(body, None, Some(capture.clone()));
    assert_eq!(
        stream.next().await.unwrap().unwrap().data.as_deref(),
        Some("one")
    );
    assert_eq!(
        stream.next().await.unwrap().unwrap().data.as_deref(),
        Some("two")
    );
    assert!(stream.next().await.is_none());
    drop(stream);
    let records = capture.test_records();
    assert_eq!(
        records
            .iter()
            .filter(|(phase, state, _)| *phase == "first_sse_event" && *state == "complete")
            .count(),
        1
    );
    assert!(records.contains(&("body", "complete", 22)));
}

#[test]
fn unconsumed_sse_is_distinct_from_successful_body_completion() {
    let capture = crate::mcp_phase_timing::Capture::new("http", Some("tools/list"), None).unwrap();
    let body =
        codex_exec_server::HttpResponseBodyStream::from_chunks(vec![b"data: one\n\n".to_vec()]);
    let stream = super::sse_stream_from_body_observed(body, None, Some(capture.clone()));
    drop(stream);
    let records = capture.test_records();
    assert!(records.contains(&("body", "cancelled", 0)));
    assert!(!records.iter().any(|(_, state, _)| *state == "complete"));
}

#[tokio::test]
async fn stalled_body_reports_received_progress_without_starting_json_decode() -> anyhow::Result<()>
{
    use codex_exec_server::HttpClient;
    use codex_exec_server::HttpRequestParams;
    use codex_exec_server::RouteAwareHttpClient;
    use codex_transport::http::HttpClientFactory;
    use codex_transport::http::OutboundProxyPolicy;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = format!("http://{}/mcp", listener.local_addr()?);
    let (chunks, receiver) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(2);
    let receiver = Arc::new(Mutex::new(Some(receiver)));
    let app = axum::Router::new().route(
        "/mcp",
        axum::routing::get(move || {
            let receiver = receiver.lock().unwrap().take().expect("one request");
            async move {
                let stream = futures::stream::unfold(receiver, |mut receiver| async {
                    receiver.recv().await.map(|chunk| (chunk, receiver))
                });
                (
                    [("content-type", "application/json")],
                    axum::body::Body::from_stream(stream),
                )
            }
        }),
    );
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await
    }));
    let client =
        RouteAwareHttpClient::new(HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault));
    crate::oauth::test_support::warm_http_client(&client).await?;
    chunks
        .send(Ok(bytes::Bytes::from(vec![b' '; 64 * 1024])))
        .await?;
    let (_, mut body) = client
        .http_request_stream(HttpRequestParams {
            method: "GET".to_string(),
            url: endpoint,
            headers: Vec::new(),
            body: None,
            timeout_ms: None,
            redirect_policy: codex_exec_server::HttpRedirectPolicy::Stop,
            request_id: "test-stalled-body".to_string(),
            stream_response: true,
        })
        .await?;
    let capture = crate::mcp_phase_timing::Capture::new("http", Some("tools/list"), None).unwrap();
    let mut pending = Box::pin(async {
        let body = super::collect_body_observed(&mut body, None, Some(&capture)).await?;
        let mut decode = capture.phase("json_decode");
        let parsed = serde_json::from_slice::<serde_json::Value>(&body);
        decode.finish(parsed.is_err());
        Ok::<
            _,
            rmcp::transport::streamable_http_client::StreamableHttpError<
                super::StreamableHttpClientAdapterError,
            >,
        >(parsed)
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                _ = &mut pending => panic!("body must remain pending before EOF"),
                _ = tokio::time::sleep(Duration::from_millis(1)) => {}
            }
            if capture
                .test_records()
                .iter()
                .any(|(phase, _, bytes)| *phase == "body_progress" && *bytes == 64 * 1024)
            {
                break;
            }
        }
    })
    .await?;
    let before_drop = capture.test_records();
    assert!(
        !before_drop
            .iter()
            .any(|(phase, _, _)| *phase == "json_decode")
    );
    assert!(!before_drop.contains(&("body", "complete", 64 * 1024)));
    drop(pending);
    assert_eq!(
        capture.test_records().last(),
        Some(&("body", "cancelled", 64 * 1024))
    );
    drop(chunks);
    server.abort();
    Ok(())
}

struct AsyncRequestAuth {
    requests: std::sync::atomic::AtomicUsize,
}

impl crate::AuthProviderResolver for AsyncRequestAuth {
    fn resolve<'a>(
        &'a self,
        _headers: &'a HeaderMap,
    ) -> futures::future::BoxFuture<
        'a,
        Result<codex_transport::api::SharedAuthProvider, codex_transport::api::AuthError>,
    > {
        Box::pin(async move {
            tokio::task::yield_now().await;
            let id = self
                .requests
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(std::sync::Arc::new(SelectedRequestAuth(id))
                as codex_transport::api::SharedAuthProvider)
        })
    }
}

struct SelectedRequestAuth(usize);

impl codex_transport::api::AuthProvider for SelectedRequestAuth {
    fn add_auth_headers(
        &self,
        headers: &mut HeaderMap,
    ) -> Result<(), codex_transport::api::AuthError> {
        headers.remove("x-openai-agent-assertion");
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("AgentAssertion proof-{}", self.0)).unwrap(),
        );
        Ok(())
    }

    fn apply_auth_headers(
        &self,
        mut headers: HeaderMap,
    ) -> codex_transport::api::AuthHeadersFuture<'_> {
        Box::pin(async move {
            self.add_auth_headers(&mut headers)?;
            Ok(headers)
        })
    }
}

#[tokio::test]
async fn requests_await_auth_and_stop_assertion_redirects() -> anyhow::Result<()> {
    use codex_exec_server::RouteAwareHttpClient;
    use codex_transport::http::HttpClientFactory;
    use codex_transport::http::OutboundProxyPolicy;
    use futures::StreamExt;
    use rmcp::transport::streamable_http_client::StreamableHttpClient;
    use std::collections::HashMap;
    use std::sync::Arc;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(202))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream"))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/redirect"))
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("location", format!("{}/leaked", server.uri())),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer stale"));
    headers.insert(
        "x-openai-agent-assertion",
        HeaderValue::from_static("stale-proof"),
    );
    let adapter = super::StreamableHttpClientAdapter::new(
        Arc::new(RouteAwareHttpClient::new(HttpClientFactory::new(
            OutboundProxyPolicy::ReqwestDefault,
        ))),
        headers,
        Some(Arc::new(AsyncRequestAuth {
            requests: Default::default(),
        })),
        /*has_configured_headers*/ false,
        StreamableHttpRedirectMode::Legacy,
        Default::default(),
    );
    let uri: Arc<str> = format!("{}/mcp", server.uri()).into();
    let notification: rmcp::model::ClientJsonRpcMessage =
        serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0", "method": "notifications/initialized",
        }))?;
    for _ in 0..2 {
        adapter
            .post_message(
                uri.clone(),
                notification.clone(),
                /*session_id*/ None,
                /*auth_token*/ None,
                HashMap::new(),
            )
            .await?;
    }
    for _ in 0..2 {
        let mut stream = adapter
            .get_stream(
                uri.clone(),
                Some("session-a".into()),
                /*last_event_id*/ None,
                /*auth_token*/ None,
                HashMap::new(),
            )
            .await?;
        assert!(stream.next().await.is_none());
    }
    adapter
        .delete_session(
            uri,
            "session-a".into(),
            /*auth_token*/ None,
            HashMap::new(),
        )
        .await?;
    assert!(
        adapter
            .post_message(
                format!("{}/redirect", server.uri()).into(),
                notification,
                /*session_id*/ None,
                /*auth_token*/ None,
                HashMap::new()
            )
            .await
            .is_err()
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 6);
    for (id, request) in requests.iter().enumerate() {
        assert_eq!(
            request.headers["authorization"],
            format!("AgentAssertion proof-{id}")
        );
        assert!(!request.headers.contains_key("x-openai-agent-assertion"));
        assert_ne!(request.url.path(), "/leaked");
    }
    server.verify().await;
    Ok(())
}
