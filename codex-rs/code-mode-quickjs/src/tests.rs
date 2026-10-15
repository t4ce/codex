use super::*;
use codex_protocol::ToolName;
use serde_json::json;
use std::sync::atomic::AtomicUsize;

#[derive(Default)]
struct Delegate {
    calls: Mutex<Vec<CodeModeNestedToolCall>>,
    closed: AtomicUsize,
    notes: Mutex<Vec<String>>,
    notification_call_ids: Mutex<Vec<String>>,
}

impl CodeModeSessionDelegate for Delegate {
    fn invoke_tool<'a>(
        &'a self,
        invocation: CodeModeNestedToolCall,
        _: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(invocation.clone());
            tokio::time::sleep(Duration::from_millis(5)).await;
            if invocation
                .input
                .as_ref()
                .and_then(|v| v.get("fail"))
                .is_some()
            {
                return Err("delegate denied tool".into());
            }
            Ok(json!({"output": invocation.input}))
        })
    }
    fn notify<'a>(
        &'a self,
        tool_call_id: String,
        _: CellId,
        text: String,
        _: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async move {
            self.notes.lock().unwrap().push(text);
            self.notification_call_ids
                .lock()
                .unwrap()
                .push(tool_call_id);
            Ok(())
        })
    }
    fn cell_closed(&self, _: &CellId) {
        self.closed.fetch_add(1, Ordering::Relaxed);
    }
}

fn tool(name: &str, kind: CodeModeToolKind) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        tool_name: ToolName::namespaced("native", "exec_command"),
        kind,
        description: "test tool".into(),
        input_schema: None,
        input_schema_max_bytes: None,
        output_schema: None,
    }
}

fn request(source: &str) -> ExecuteRequest {
    ExecuteRequest {
        tool_call_id: "request-1".into(),
        enabled_tools: vec![tool("exec_command", CodeModeToolKind::Function)],
        source: source.into(),
        yield_time_ms: Some(1_000),
        max_output_tokens: Some(1_000),
    }
}

async fn execute(
    session: &Arc<dyn CodeModeSession>,
    request: ExecuteRequest,
    delegate: Arc<Delegate>,
) -> RuntimeResponse {
    tokio::time::timeout(Duration::from_secs(3), async {
        session
            .execute(request, delegate, None)
            .await
            .unwrap()
            .initial_response()
            .await
            .unwrap()
    })
    .await
    .expect("real JavaScript execution timed out")
}

fn texts(response: RuntimeResponse) -> Vec<String> {
    match response {
        RuntimeResponse::Result {
            content_items,
            error_text,
            ..
        } => {
            assert_eq!(error_text, None);
            content_items
                .into_iter()
                .map(|item| match item {
                    FunctionCallOutputContentItem::InputText { text } => text,
                    _ => panic!("unexpected content"),
                })
                .collect()
        }
        other => panic!("expected completed execution, got {other:?}"),
    }
}

#[tokio::test]
async fn real_modules_await_parallel_tools_and_preserve_authoritative_tool_identity() {
    let session = QuickJsCodeModeSessionProvider
        .create_session()
        .await
        .unwrap();
    let delegate = Arc::new(Delegate::default());
    let result=execute(&session,request(r#"
        const {output} = await tools.exec_command({cmd:'gri', computed: 3*7});
        const results = await Promise.all([tools.exec_command({n:1}),tools.exec_command({n:2})]);
        text({output, sum:results.map(x=>x.output.n).reduce((a,b)=>a+b), tools:ALL_TOOLS.map(x=>x.name)});
    "#),delegate.clone()).await;
    assert_eq!(
        texts(result),
        vec![r#"{"output":{"cmd":"gri","computed":21},"sum":3,"tools":["exec_command"]}"#]
    );
    let calls = delegate.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|call| call.tool_name
        == ToolName::namespaced("native", "exec_command")
        && call.tool_kind == CodeModeToolKind::Function));
    assert_eq!(delegate.closed.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn store_load_persist_within_session_and_are_isolated_between_sessions() {
    let provider = QuickJsCodeModeSessionProvider;
    let a = provider.create_session().await.unwrap();
    let b = provider.create_session().await.unwrap();
    let delegate = Arc::new(Delegate::default());
    assert!(
        texts(
            execute(
                &a,
                request("let x=40; await Promise.resolve();store('x',{value:x+2});"),
                delegate.clone()
            )
            .await
        )
        .is_empty()
    );
    assert_eq!(
        texts(
            execute(
                &a,
                request("const saved=load('x');text(saved.value);text(typeof x);"),
                delegate.clone()
            )
            .await
        ),
        vec!["42", "undefined"]
    );
    assert_eq!(
        texts(execute(&b, request("text(load('x'));"), delegate).await),
        vec!["undefined"]
    );
}

#[tokio::test]
async fn tool_rejections_are_real_javascript_exceptions_and_freeform_is_preserved() {
    let session = QuickJsCodeModeSessionProvider
        .create_session()
        .await
        .unwrap();
    let delegate = Arc::new(Delegate::default());
    let result = execute(
        &session,
        request("try{await tools.exec_command({fail:true})}catch(e){text(e.message)}"),
        delegate.clone(),
    )
    .await;
    assert_eq!(texts(result), vec!["delegate denied tool"]);
    let mut freeform = request("const r=await tools.raw('gri');text(r.output)");
    freeform.enabled_tools = vec![tool("raw", CodeModeToolKind::Freeform)];
    assert_eq!(
        texts(execute(&session, freeform, delegate.clone()).await),
        vec!["gri"]
    );
    let calls = delegate.calls.lock().unwrap();
    assert_eq!(calls[1].tool_kind, CodeModeToolKind::Freeform);
    assert_eq!(calls[1].input, Some(json!("gri")));
}

#[tokio::test]
async fn timers_and_settled_helpers_run_real_promise_jobs() {
    let session = QuickJsCodeModeSessionProvider
        .create_session()
        .await
        .unwrap();
    let delegate = Arc::new(Delegate::default());
    let result=execute(&session,request("await new Promise(r=>setTimeout(r,5));for await(const result of as_settled([Promise.resolve(4),Promise.resolve(5)])){text(result.value)}"),delegate).await;
    assert_eq!(texts(result), vec!["4", "5"]);
}

#[tokio::test]
async fn native_argument_coercions_can_reenter_host_callbacks() {
    let session = QuickJsCodeModeSessionProvider
        .create_session()
        .await
        .unwrap();
    let delegate = Arc::new(Delegate::default());
    let result = execute(
        &session,
        request(
            r#"
        const originalString = String;
        globalThis.String = value => value === 'outer' ? {
            toString() { store('inner', 17); text('string-reentered'); return 'outer'; }
        } : originalString(value);
        store('outer', 42);
        globalThis.String = originalString;
        text([load('inner'), load('outer')]);

        const originalStringify = JSON.stringify;
        JSON.stringify = value => value === 43 ? {
            toString() { text('json-reentered'); return '43'; }
        } : originalStringify(value);
        store('json', 43);
        JSON.stringify = originalStringify;
        text(load('json'));

        const originalNumber = Number;
        globalThis.Number = () => ({ valueOf() { text('number-reentered'); return 0; } });
        await new Promise(resolve => setTimeout(resolve, 0));
        globalThis.Number = originalNumber;
        text('timer-done');
    "#,
        ),
        delegate,
    )
    .await;
    assert_eq!(
        texts(result),
        vec![
            "string-reentered",
            "[17,42]",
            "json-reentered",
            "43",
            "number-reentered",
            "timer-done",
        ]
    );
}

#[tokio::test]
async fn bootstrap_metadata_and_call_id_are_data_even_when_they_contain_placeholders() {
    let session = QuickJsCodeModeSessionProvider
        .create_session()
        .await
        .unwrap();
    let delegate = Arc::new(Delegate::default());
    let mut req = request(
        "text(ALL_TOOLS[0].description); notify('literal call id'); await tools.exec_command({ok:true});",
    );
    req.enabled_tools[0].description = "literal __CALL_ID__ and __METADATA__".into();
    req.tool_call_id = "literal __METADATA__ and __CALL_ID__".into();
    assert_eq!(
        texts(execute(&session, req, delegate.clone()).await),
        vec!["literal __CALL_ID__ and __METADATA__"]
    );
    assert_eq!(*delegate.notes.lock().unwrap(), vec!["literal call id"]);
    assert_eq!(
        *delegate.notification_call_ids.lock().unwrap(),
        vec!["literal __METADATA__ and __CALL_ID__"]
    );
    assert_eq!(delegate.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn embedded_nul_text_and_store_keys_preserve_their_full_javascript_strings() {
    let session = QuickJsCodeModeSessionProvider
        .create_session()
        .await
        .unwrap();
    let delegate = Arc::new(Delegate::default());
    let result = execute(
        &session,
        request(
            r#"
        text('before\u0000after');
        store('key', 1);
        store('key\u0000suffix', 2);
        text([load('key'), load('key\u0000suffix')]);
    "#,
        ),
        delegate,
    )
    .await;
    assert_eq!(texts(result), vec!["before\0after", "[1,2]"]);
}

#[tokio::test]
async fn clear_timeout_cannot_cancel_a_tool_with_the_same_internal_id() {
    let session = QuickJsCodeModeSessionProvider
        .create_session()
        .await
        .unwrap();
    let delegate = Arc::new(Delegate::default());
    let result = execute(
        &session,
        request(
            r#"
        const pending = tools.exec_command({value: 42});
        clearTimeout('1');
        text((await pending).output.value);
    "#,
        ),
        delegate,
    )
    .await;
    assert_eq!(texts(result), vec!["42"]);
}

#[tokio::test(flavor = "current_thread")]
async fn cleared_timer_tasks_remain_bounded_until_the_runtime_reaps_them() {
    let session = Arc::new(Session {
        cells: Mutex::new(HashMap::new()),
        values: Arc::new(Mutex::new(HashMap::new())),
        next: AtomicU64::new(1),
        shutdown: CancellationToken::new(),
        limits: CodeModeSessionCellExecutionLimits::default(),
    });
    let delegate = Arc::new(Delegate::default());
    let started = session
        .execute(
            request(
                r#"
        let result = 'unexpected completion';
        try { for(let i=0;i<10000;i++){clearTimeout(setTimeout(()=>{},86400000));} }
        catch(error) { result = error.message; }
        store('timer-loop-result', result);
        text('done');
    "#,
            ),
            delegate.clone(),
            None,
        )
        .await
        .unwrap();
    // Deliberately leave this single-thread Tokio runtime unpolled while the
    // interpreter runs. Aborted timers cannot finish yet and must count toward
    // the bound, even though every JS callback has already been cleared.
    let deadline = Instant::now() + Duration::from_secs(2);
    let error = loop {
        if let Some(result) = session
            .values
            .lock()
            .unwrap()
            .get("timer-loop-result")
            .cloned()
        {
            break result;
        }
        assert!(Instant::now() < deadline, "timer loop failed to finish");
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(
        error.as_str().unwrap().contains("pending host task limit"),
        "{error}"
    );
    let result = tokio::time::timeout(Duration::from_secs(2), started.initial_response())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(texts(result), vec!["done"]);
    assert_eq!(delegate.closed.load(Ordering::Relaxed), 1);
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn syntax_and_unhandled_rejection_fail_honestly_without_privileged_globals() {
    let session = QuickJsCodeModeSessionProvider
        .create_session()
        .await
        .unwrap();
    let delegate = Arc::new(Delegate::default());
    for source in [
        "let = ;",
        "await Promise.reject(new Error('rejected'));",
        "Promise.reject(new Error('unhandled'));",
        "tools.not_enabled({})",
        "image('x')",
    ] {
        let result = execute(&session, request(source), delegate.clone()).await;
        assert!(
            matches!(
                result,
                RuntimeResponse::Result {
                    error_text: Some(_),
                    ..
                }
            ),
            "{result:?}"
        );
    }
    assert_eq!(texts(execute(&session,request("text([typeof console,typeof require,typeof fetch,typeof process,typeof Atomics,typeof WebAssembly])"),delegate).await),vec![r#"["undefined","undefined","undefined","undefined","undefined","undefined"]"#]);
}

#[tokio::test]
async fn infinite_loop_yields_observation_and_can_be_cancelled_and_cleaned_up() {
    let session = QuickJsCodeModeSessionProvider
        .create_session()
        .await
        .unwrap();
    let delegate = Arc::new(Delegate::default());
    let mut looping = request("while(true){}");
    looping.yield_time_ms = Some(5);
    let started = session
        .execute(looping, delegate.clone(), None)
        .await
        .unwrap();
    let id = started.cell_id.clone();
    let response = tokio::time::timeout(Duration::from_secs(2), started.initial_response())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(response, RuntimeResponse::Yielded { .. }));
    let stopped = tokio::time::timeout(Duration::from_secs(2), session.terminate(id))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        stopped,
        WaitOutcome::LiveCell(RuntimeResponse::Terminated { .. })
    ));
    assert_eq!(delegate.closed.load(Ordering::Relaxed), 1);
    session.shutdown().await.unwrap();
    assert!(
        session
            .execute(request("text('late')"), delegate, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn preemption_keeps_the_real_cell_alive_until_wait_and_heap_limits_are_enforced() {
    let session = QuickJsCodeModeSessionProvider
        .create_session()
        .await
        .unwrap();
    let delegate = Arc::new(Delegate::default());
    let preempt = CancellationToken::new();
    preempt.cancel();
    let started = session
        .execute(
            request("await new Promise(r=>setTimeout(r,20));text('done')"),
            delegate.clone(),
            Some(preempt),
        )
        .await
        .unwrap();
    let id = started.cell_id.clone();
    assert!(matches!(
        started.initial_response().await.unwrap(),
        RuntimeResponse::Yielded { .. }
    ));
    let result = session
        .wait(
            WaitRequest {
                cell_id: id,
                yield_time_ms: 1000,
            },
            None,
        )
        .await
        .unwrap();
    let WaitOutcome::LiveCell(result) = result else {
        panic!()
    };
    assert_eq!(texts(result), vec!["done"]);
    let limited = QuickJsCodeModeSessionProvider
        .create_session_with_limits(CodeModeSessionCellExecutionLimits {
            max_heap_size_bytes: Some(512 * 1024),
            max_yield_time_ms: None,
        })
        .await
        .unwrap();
    let result = execute(
        &limited,
        request("const huge = new Array(1000000).fill('memory');text(huge.length)"),
        delegate,
    )
    .await;
    assert!(
        matches!(
            result,
            RuntimeResponse::Result {
                error_text: Some(_),
                ..
            }
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn explicit_yield_retains_the_cell_and_notifications_finish_before_cleanup() {
    let session = QuickJsCodeModeSessionProvider
        .create_session()
        .await
        .unwrap();
    let delegate = Arc::new(Delegate::default());
    let started = session.execute(request("text('before');yield_control();await new Promise(r=>setTimeout(r,30));notify({hello:3});text('after');"),delegate.clone(),None).await.unwrap();
    let id = started.cell_id.clone();
    let initial = started.initial_response().await.unwrap();
    assert!(
        matches!(initial, RuntimeResponse::Yielded { .. }),
        "{initial:?}"
    );
    let RuntimeResponse::Yielded { content_items, .. } = initial else {
        unreachable!()
    };
    assert_eq!(
        content_items,
        vec![FunctionCallOutputContentItem::InputText {
            text: "before".into()
        }]
    );
    let WaitOutcome::LiveCell(result) = session
        .wait(
            WaitRequest {
                cell_id: id,
                yield_time_ms: 1000,
            },
            None,
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(texts(result), vec!["after"]);
    assert_eq!(*delegate.notes.lock().unwrap(), vec![r#"{"hello":3}"#]);
    assert_eq!(
        texts(
            execute(
                &session,
                request("try{exit()}catch{};text('unreachable')"),
                delegate
            )
            .await
        ),
        Vec::<String>::new()
    );
}

#[derive(Default)]
struct BlockingDelegate {
    active: Arc<AtomicUsize>,
    closed: AtomicUsize,
}
struct ActiveCall(Arc<AtomicUsize>);
impl Drop for ActiveCall {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
impl CodeModeSessionDelegate for BlockingDelegate {
    fn invoke_tool<'a>(
        &'a self,
        _: CodeModeNestedToolCall,
        cancel: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async move {
            self.active.fetch_add(1, Ordering::Relaxed);
            let _guard = ActiveCall(self.active.clone());
            cancel.cancelled().await;
            Err("cancelled".into())
        })
    }
    fn notify<'a>(
        &'a self,
        _: String,
        _: CellId,
        _: String,
        _: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async { Ok(()) })
    }
    fn cell_closed(&self, _: &CellId) {
        self.closed.fetch_add(1, Ordering::Relaxed);
    }
}

struct PanickingCloseDelegate;
impl CodeModeSessionDelegate for PanickingCloseDelegate {
    fn invoke_tool<'a>(
        &'a self,
        _: CodeModeNestedToolCall,
        _: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async { Ok(json!(null)) })
    }
    fn notify<'a>(
        &'a self,
        _: String,
        _: CellId,
        _: String,
        _: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async { Ok(()) })
    }
    fn cell_closed(&self, _: &CellId) {
        panic!("test close callback panic");
    }
}

#[tokio::test]
async fn close_callback_panic_does_not_strand_cell_completion_or_shutdown() {
    let session = QuickJsCodeModeSessionProvider
        .create_session()
        .await
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), async {
        session
            .execute(
                request("text('finished')"),
                Arc::new(PanickingCloseDelegate),
                None,
            )
            .await
            .unwrap()
            .initial_response()
            .await
            .unwrap()
    })
    .await
    .expect("cell completion was lost after close callback panic");
    assert!(matches!(response, RuntimeResponse::Result { .. }));
    tokio::time::timeout(Duration::from_secs(2), session.shutdown())
        .await
        .expect("shutdown waited for a lost completion")
        .unwrap();
}

#[tokio::test]
async fn shutdown_cancels_and_releases_pending_delegate_calls_before_reporting_closed() {
    let session = QuickJsCodeModeSessionProvider
        .create_session()
        .await
        .unwrap();
    let delegate = Arc::new(BlockingDelegate::default());
    let mut req = request("await tools.exec_command({cmd:'gri'})");
    req.yield_time_ms = Some(1);
    let started = session.execute(req, delegate.clone(), None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while delegate.active.load(Ordering::Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        started.initial_response().await.unwrap(),
        RuntimeResponse::Yielded { .. }
    ));
    tokio::time::timeout(Duration::from_secs(2), session.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delegate.active.load(Ordering::Relaxed), 0);
    assert_eq!(delegate.closed.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn output_limit_rejects_excess_text_without_leaking_it_to_the_delegate() {
    let session = QuickJsCodeModeSessionProvider
        .create_session()
        .await
        .unwrap();
    let delegate = Arc::new(Delegate::default());
    let mut req = request("text('123456789')");
    req.max_output_tokens = Some(2);
    let result = execute(&session, req, delegate.clone()).await;
    assert!(
        matches!(result,RuntimeResponse::Result{content_items,error_text:Some(error),code_mode_host_duration:Some(_),..} if content_items.is_empty()&&error.contains("output limit"))
    );
    assert!(delegate.calls.lock().unwrap().is_empty());
}
