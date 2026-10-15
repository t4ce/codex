//! QuickJS owns all JS values on one execution thread. Only JSON, cancellation
//! and protocol output cross threads; nested tools run through the existing delegate.
#![allow(unsafe_op_in_unsafe_fn)]
use std::collections::HashMap;
use std::ffi::CString;
use std::ffi::c_char;
use std::ffi::c_int;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use codex_code_mode_protocol::*;
use serde_json::Value;
use serde_json::json;
use trueos_qjs as q;

use crate::Cell;

unsafe extern "C" {
    fn native_engine_js_free_value(ctx: *mut q::JSContext, value: q::JSValue);
    fn native_engine_js_dup_value(ctx: *mut q::JSContext, value: q::JSValue) -> q::JSValue;
    fn JS_SetInterruptHandler(
        rt: *mut q::JSRuntime,
        cb: unsafe extern "C" fn(*mut q::JSRuntime, *mut c_void) -> c_int,
        opaque: *mut c_void,
    );
    fn JS_SetMemoryLimit(rt: *mut q::JSRuntime, limit: usize);
    fn JS_SetMaxStackSize(rt: *mut q::JSRuntime, limit: usize);
    fn JS_PromiseState(ctx: *mut q::JSContext, promise: q::JSValue) -> c_int;
    fn JS_PromiseResult(ctx: *mut q::JSContext, promise: q::JSValue) -> q::JSValue;
    fn JS_ParseJSON(
        ctx: *mut q::JSContext,
        data: *const c_char,
        len: usize,
        filename: *const c_char,
    ) -> q::JSValue;
}

enum Message {
    Tool(u64, Result<Value, String>),
    Timer(u64),
    Notification(Result<(), String>),
}

struct Bridge {
    cell: Arc<Cell>,
    delegate: Arc<dyn CodeModeSessionDelegate>,
    runtime: tokio::runtime::Handle,
    values: Arc<Mutex<HashMap<String, Value>>>,
    tools: Vec<ToolDefinition>,
    next: u64,
    promises: HashMap<u64, [q::JSValue; 2]>,
    timers: HashMap<u64, q::JSValue>,
    tx: mpsc::Sender<Message>,
    last_yield: Instant,
    exit: bool,
    max_output_bytes: usize,
    output_bytes: usize,
    callbacks_cancel: tokio_util::sync::CancellationToken,
    tasks: HashMap<u64, tokio::task::JoinHandle<()>>,
    pending_notifications: usize,
    unhandled: HashMap<usize, String>,
    max_store_bytes: usize,
}

unsafe extern "C" fn interrupt(_: *mut q::JSRuntime, opaque: *mut c_void) -> c_int {
    let state = opaque.cast::<Bridge>();
    if (*state).last_yield.elapsed() >= Duration::from_millis(1) {
        std::thread::yield_now();
        (*state).last_yield = Instant::now();
    }
    c_int::from((*state).exit || (&(*state).cell).cancel.is_cancelled())
}

unsafe fn string(ctx: *mut q::JSContext, value: q::JSValue) -> Result<String, String> {
    let mut len = 0;
    let ptr = q::JS_ToCStringLen2(ctx, &mut len, value, 0);
    if ptr.is_null() {
        return Err("JavaScript string conversion failed".into());
    }
    let result =
        String::from_utf8_lossy(std::slice::from_raw_parts(ptr.cast::<u8>(), len)).into_owned();
    q::JS_FreeCString(ctx, ptr);
    Ok(result)
}

unsafe fn js_string(ctx: *mut q::JSContext, value: &str) -> q::JSValue {
    q::JS_NewStringLen(ctx, value.as_ptr().cast(), value.len())
}

unsafe fn exception(ctx: *mut q::JSContext) -> String {
    let error = q::JS_GetException(ctx);
    let text = string(ctx, error).unwrap_or_else(|_| "QuickJS exception".into());
    native_engine_js_free_value(ctx, error);
    text
}

unsafe fn throw(ctx: *mut q::JSContext, message: &str) -> q::JSValue {
    let error = q::JS_NewError(ctx);
    q::JS_SetPropertyStr(ctx, error, c"message".as_ptr(), js_string(ctx, message));
    q::JS_Throw(ctx, error)
}

unsafe extern "C" fn promise_rejection(
    ctx: *mut q::JSContext,
    promise: q::JSValue,
    reason: q::JSValue,
    handled: c_int,
    opaque: *mut c_void,
) {
    // Called only by this interpreter thread. Delay reporting until its job
    // queue drains so attaching a rejection handler in a microtask can recover.
    let key = promise.u.ptr as usize;
    if handled != 0 {
        (&mut *opaque.cast::<Bridge>()).unhandled.remove(&key);
    } else {
        let error = string(ctx, reason).unwrap_or_else(|_| "unhandled promise rejection".into());
        (&mut *opaque.cast::<Bridge>()).unhandled.insert(key, error);
    }
}

unsafe extern "C" fn native(
    ctx: *mut q::JSContext,
    _: q::JSValue,
    argc: c_int,
    argv: *const q::JSValue,
) -> q::JSValue {
    // Rust panics must never unwind through QuickJS' C stack.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dispatch(ctx, argc, argv))) {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => throw(ctx, &error),
        Err(_) => throw(ctx, "QuickJS host callback panicked"),
    }
}

unsafe fn ready(state: *mut Bridge) -> Result<(), String> {
    if (*state).exit {
        return Err("execution exited".into());
    }
    if (&(*state).cell).cancel.is_cancelled() {
        return Err("execution cancelled".into());
    }
    Ok(())
}

unsafe fn host_task_available(state: *mut Bridge) -> Result<(), String> {
    ready(state)?;
    (&mut (*state).tasks).retain(|_, task| !task.is_finished());
    if (*state).tasks.len() >= 128 {
        return Err("QuickJS pending host task limit exceeded".into());
    }
    Ok(())
}

unsafe fn dispatch(
    ctx: *mut q::JSContext,
    argc: c_int,
    argv: *const q::JSValue,
) -> Result<q::JSValue, String> {
    let args = std::slice::from_raw_parts(argv, argc.max(0) as usize);
    let arg = |index| {
        args.get(index)
            .copied()
            .unwrap_or_else(q::JSValue::undefined)
    };
    let state = q::JS_GetContextOpaque(ctx).cast::<Bridge>();
    let op = string(ctx, arg(0))?;
    ready(state)?;
    // Coercions and QuickJS allocation/free can reenter JS and native callbacks.
    // Keep only the raw state pointer across those calls, never a Rust borrow
    // of Bridge or its maps. Converted inputs and removed JS values are owned.
    match op.as_str() {
        "text" => {
            let text = string(ctx, arg(1))?;
            ready(state)?;
            (*state).output_bytes = (*state).output_bytes.saturating_add(text.len());
            if (*state).output_bytes > (*state).max_output_bytes {
                return Err("QuickJS output limit exceeded".into());
            }
            (*state)
                .cell
                .output(FunctionCallOutputContentItem::InputText { text });
            Ok(q::JSValue::undefined())
        }
        "store" => {
            let key = string(ctx, arg(1))?;
            let serialized = string(ctx, arg(2))?;
            ready(state)?;
            let limit = (*state).max_store_bytes;
            if key.len() > 4096 || serialized.len() > limit {
                return Err("QuickJS store limit exceeded".into());
            }
            let value = serde_json::from_str(&serialized)
                .map_err(|e| format!("store requires JSON data: {e}"))?;
            let mut values = (*state).values.lock().unwrap();
            let retained: usize = values
                .iter()
                .filter(|(name, _)| *name != &key)
                .map(|(name, value)| name.len().saturating_add(value.to_string().len()))
                .sum();
            if retained
                .saturating_add(key.len())
                .saturating_add(serialized.len())
                > limit
            {
                return Err("QuickJS store limit exceeded".into());
            }
            values.insert(key, value);
            Ok(q::JSValue::undefined())
        }
        "load" => {
            let key = string(ctx, arg(1))?;
            ready(state)?;
            let value = {
                (*state)
                    .values
                    .lock()
                    .unwrap()
                    .get(&key)
                    .map(Value::to_string)
            };
            Ok(value
                .map(|value| js_string(ctx, &value))
                .unwrap_or_else(q::JSValue::undefined))
        }
        "exit" => {
            (*state).exit = true;
            Ok(q::JSValue::undefined())
        }
        "yield" => {
            (*state).cell.yield_control();
            Ok(q::JSValue::undefined())
        }
        "tool" => {
            let index = string(ctx, arg(1))?
                .parse::<usize>()
                .map_err(|_| "invalid tool index")?;
            let input = if arg(2).tag == q::JS_TAG_UNDEFINED {
                None
            } else {
                Some(
                    serde_json::from_str(&string(ctx, arg(2))?)
                        .map_err(|e| format!("invalid tool input: {e}"))?,
                )
            };
            host_task_available(state)?;
            if (*state).promises.len() >= 128 {
                return Err("QuickJS pending tool limit exceeded".into());
            }
            let tool = (&(*state).tools)
                .get(index)
                .ok_or("tool is not enabled in this cell")?
                .clone();
            let id = (*state).next;
            (*state).next += 1;
            let mut resolving = [q::JSValue::undefined(); 2];
            let promise = q::JS_NewPromiseCapability(ctx, resolving.as_mut_ptr());
            if promise.is_exception() {
                return Err(exception(ctx));
            }
            (&mut (*state).promises).insert(id, resolving);
            let invocation = CodeModeNestedToolCall {
                cell_id: (&(*state).cell).id.clone(),
                runtime_tool_call_id: format!("quickjs-tool-{id}"),
                tool_name: tool.tool_name,
                tool_kind: tool.kind,
                input,
            };
            let delegate = (*state).delegate.clone();
            let tx = (*state).tx.clone();
            let cancel = (*state).callbacks_cancel.clone();
            let runtime = (*state).runtime.clone();
            let task=runtime.spawn(async move{
                let result=tokio::select!{_ = cancel.cancelled()=>return,result=delegate.invoke_tool(invocation,cancel.clone())=>result};
                let _=tx.send(Message::Tool(id,result));
            });
            (&mut (*state).tasks).insert(id, task);
            Ok(promise)
        }
        "notify" => {
            let text = string(ctx, arg(1))?;
            let call_id = string(ctx, arg(2))?;
            host_task_available(state)?;
            if (*state).pending_notifications >= 128 {
                return Err("QuickJS pending notification limit exceeded".into());
            }
            let delegate = (*state).delegate.clone();
            let cancel = (*state).callbacks_cancel.clone();
            let cell_id = (&(*state).cell).id.clone();
            let tx = (*state).tx.clone();
            let runtime = (*state).runtime.clone();
            let id = (*state).next;
            (*state).next += 1;
            (*state).pending_notifications += 1;
            let task=runtime.spawn(async move{
                let result=tokio::select!{_ = cancel.cancelled()=>return,result=delegate.notify(call_id,cell_id,text,cancel.clone())=>result};
                let _=tx.send(Message::Notification(result));
            });
            (&mut (*state).tasks).insert(id, task);
            Ok(q::JSValue::undefined())
        }
        "timer" => {
            let mut millis = 0.0;
            if q::JS_ToFloat64(ctx, &mut millis, arg(2)) < 0 {
                return Err(exception(ctx));
            }
            host_task_available(state)?;
            if (*state).timers.len() >= 128 {
                return Err("QuickJS pending timer limit exceeded".into());
            }
            let callback = native_engine_js_dup_value(ctx, arg(1));
            let id = (*state).next;
            (*state).next += 1;
            (&mut (*state).timers).insert(id, callback);
            let tx = (*state).tx.clone();
            let cancel = (*state).callbacks_cancel.clone();
            let runtime = (*state).runtime.clone();
            let delay = Duration::from_millis(millis.max(0.0).min(86_400_000.0) as u64);
            let task=runtime.spawn(async move{
                tokio::select!{_ = cancel.cancelled()=>{},_ = tokio::time::sleep(delay)=>{let _=tx.send(Message::Timer(id));}}
            });
            (&mut (*state).tasks).insert(id, task);
            Ok(js_string(ctx, &id.to_string()))
        }
        "clear_timer" => {
            if let Ok(id) = string(ctx, arg(1))?.parse::<u64>() {
                ready(state)?;
                let callback = (&mut (*state).timers).remove(&id);
                if let Some(callback) = callback {
                    if let Some(task) = (&(*state).tasks).get(&id) {
                        task.abort();
                    }
                    native_engine_js_free_value(ctx, callback);
                }
            }
            Ok(q::JSValue::undefined())
        }
        _ => Err(format!("unsupported QuickJS host operation {op}")),
    }
}

struct Engine {
    rt: *mut q::JSRuntime,
    ctx: *mut q::JSContext,
    bridge: *mut Bridge,
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            let state = self.bridge;
            (*state).callbacks_cancel.cancel();
            let tasks = std::mem::take(&mut (*state).tasks);
            for task in tasks.values() {
                task.abort();
            }
            let runtime = (*state).runtime.clone();
            runtime.block_on(async move {
                for task in tasks.into_values() {
                    let _ = task.await;
                }
            });
            let promises = std::mem::take(&mut (*state).promises);
            let timers = std::mem::take(&mut (*state).timers);
            for pair in promises.into_values() {
                for value in pair {
                    native_engine_js_free_value(self.ctx, value);
                }
            }
            for callback in timers.into_values() {
                native_engine_js_free_value(self.ctx, callback);
            }
            q::JS_FreeContext(self.ctx);
            q::JS_FreeRuntime(self.rt);
            drop(Box::from_raw(state));
        }
    }
}

pub(super) fn run(
    request: ExecuteRequest,
    delegate: Arc<dyn CodeModeSessionDelegate>,
    runtime: tokio::runtime::Handle,
    values: Arc<Mutex<HashMap<String, Value>>>,
    cell: Arc<Cell>,
    heap_limit: usize,
) -> Result<(), String> {
    unsafe {
        let rt = q::JS_NewRuntime();
        if rt.is_null() {
            return Err("QuickJS runtime allocation failed".into());
        }
        JS_SetMemoryLimit(rt, heap_limit);
        JS_SetMaxStackSize(rt, 1024 * 1024);
        let ctx = q::JS_NewContext(rt);
        if ctx.is_null() {
            q::JS_FreeRuntime(rt);
            return Err("QuickJS context allocation failed".into());
        }
        let (tx, rx) = mpsc::channel();
        let metadata: Vec<_> = request.enabled_tools.iter().map(|tool| json!({
            "name": normalize_code_mode_identifier(&tool.name), "description": tool.description,
        })).collect();
        let callbacks_cancel = cell.cancel.child_token();
        let engine = Engine {
            rt,
            ctx,
            bridge: Box::into_raw(Box::new(Bridge {
                callbacks_cancel,
                tasks: HashMap::new(),
                pending_notifications: 0,
                unhandled: HashMap::new(),
                max_store_bytes: heap_limit.min(16 * 1024 * 1024),
                cell,
                delegate,
                runtime,
                values,
                tools: request.enabled_tools,
                next: 1,
                promises: HashMap::new(),
                timers: HashMap::new(),
                tx,
                last_yield: Instant::now(),
                exit: false,
                max_output_bytes: request
                    .max_output_tokens
                    .unwrap_or(10_000)
                    .saturating_mul(4)
                    .max(1),
                output_bytes: 0,
            })),
        };
        let state = engine.bridge;
        let opaque = state.cast();
        q::JS_SetContextOpaque(ctx, opaque);
        JS_SetInterruptHandler(rt, interrupt, opaque);
        q::JS_SetHostPromiseRejectionTracker(rt, Some(promise_rejection), opaque);
        let global = q::JS_GetGlobalObject(ctx);
        let callback = q::JS_NewCFunction2(
            ctx,
            Some(native),
            c"__codeModeNative".as_ptr(),
            3,
            q::JS_CFUNC_GENERIC,
            0,
        );
        if q::JS_SetPropertyStr(ctx, global, c"__codeModeNative".as_ptr(), callback) < 0 {
            native_engine_js_free_value(ctx, global);
            return Err(exception(ctx));
        }
        native_engine_js_free_value(ctx, global);
        let boot = format!(
            "{BOOTSTRAP}({}, {});",
            serde_json::to_string(&metadata).unwrap(),
            serde_json::to_string(&request.tool_call_id).unwrap()
        );
        let initialized = q::js_eval_bytes(
            ctx,
            boot.as_bytes(),
            c"<code-mode-bootstrap>".as_ptr(),
            q::JS_EVAL_TYPE_GLOBAL,
        );
        if initialized.is_exception() {
            return Err(exception(ctx));
        }
        native_engine_js_free_value(ctx, initialized);
        let helpers = include_str!("../../code-mode-runtime/src/runtime/settled.js");
        let initialized = q::js_eval_bytes(
            ctx,
            helpers.as_bytes(),
            c"<code-mode-settled>".as_ptr(),
            q::JS_EVAL_TYPE_GLOBAL,
        );
        if initialized.is_exception() {
            return Err(exception(ctx));
        }
        native_engine_js_free_value(ctx, initialized);
        let promise = q::js_eval_bytes(
            ctx,
            request.source.as_bytes(),
            c"<code-mode-cell.mjs>".as_ptr(),
            q::JS_EVAL_TYPE_MODULE,
        );
        if promise.is_exception() {
            if (*state).exit {
                return Ok(());
            }
            return Err(exception(ctx));
        }
        let result = (|| -> Result<(), String> {
            loop {
                if (&(*state).cell).cancel.is_cancelled() {
                    return Err("execution cancelled".into());
                }
                if (*state).exit {
                    return Ok(());
                }
                let mut job_ctx = ctx;
                // Bound each job-pump pass even when promises continuously schedule jobs.
                for _ in 0..64 {
                    if q::JS_IsJobPending(rt) == 0 {
                        break;
                    }
                    if q::JS_ExecutePendingJob(rt, &mut job_ctx) < 0 {
                        if (*state).exit {
                            return Ok(());
                        }
                        return Err(exception(job_ctx));
                    }
                    if (&(*state).cell).cancel.is_cancelled() {
                        return Err("execution cancelled".into());
                    }
                }
                std::thread::yield_now();
                match JS_PromiseState(ctx, promise) {
                    1 if (*state).pending_notifications == 0 => {
                        if let Some(error) = (&(*state).unhandled).values().next() {
                            return Err(error.clone());
                        }
                        return Ok(());
                    }
                    2 => {
                        let value = JS_PromiseResult(ctx, promise);
                        let error =
                            string(ctx, value).unwrap_or_else(|_| "JavaScript rejected".into());
                        native_engine_js_free_value(ctx, value);
                        return Err(error);
                    }
                    _ => {}
                }
                if q::JS_IsJobPending(rt) != 0 {
                    continue;
                }
                let message = match rx.recv_timeout(Duration::from_millis(1)) {
                    Ok(message) => message,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => return Err("QuickJS command channel closed".into()),
                };
                match message {
                    Message::Tool(id, result) => {
                        let Some(pair) = (&mut (*state).promises).remove(&id) else {
                            continue;
                        };
                        let (index, value) = match result {
                            Ok(value) => {
                                let data = CString::new(value.to_string())
                                    .expect("JSON encodes NUL characters");
                                (
                                    0,
                                    JS_ParseJSON(
                                        ctx,
                                        data.as_ptr(),
                                        data.as_bytes().len(),
                                        c"<tool-result>".as_ptr(),
                                    ),
                                )
                            }
                            Err(error) => {
                                let value = q::JS_NewError(ctx);
                                q::JS_SetPropertyStr(
                                    ctx,
                                    value,
                                    c"message".as_ptr(),
                                    js_string(ctx, &error),
                                );
                                (1, value)
                            }
                        };
                        if value.is_exception() {
                            for resolver in pair {
                                native_engine_js_free_value(ctx, resolver);
                            }
                            return Err(exception(ctx));
                        }
                        let called =
                            q::JS_Call(ctx, pair[index], q::JSValue::undefined(), 1, &value);
                        native_engine_js_free_value(ctx, value);
                        for resolver in pair {
                            native_engine_js_free_value(ctx, resolver);
                        }
                        if called.is_exception() {
                            return Err(exception(ctx));
                        }
                        native_engine_js_free_value(ctx, called);
                    }
                    Message::Notification(result) => {
                        (*state).pending_notifications -= 1;
                        result?;
                    }
                    Message::Timer(id) => {
                        let callback = (&mut (*state).timers).remove(&id);
                        if let Some(callback) = callback {
                            let called = q::JS_Call(
                                ctx,
                                callback,
                                q::JSValue::undefined(),
                                0,
                                std::ptr::null(),
                            );
                            native_engine_js_free_value(ctx, callback);
                            if called.is_exception() {
                                return Err(exception(ctx));
                            }
                            native_engine_js_free_value(ctx, called);
                        }
                    }
                }
            }
        })();
        native_engine_js_free_value(ctx, promise);
        result
    }
}

const BOOTSTRAP: &str = r#"
(function(metadata, callId) {
 const native = globalThis.__codeModeNative;
 delete globalThis.__codeModeNative;
 for (const name of ['console','Atomics','SharedArrayBuffer','WebAssembly']) delete globalThis[name];
 globalThis.ALL_TOOLS = metadata;
 globalThis.tools = Object.create(null);
 ALL_TOOLS.forEach((metadata,index) => Object.defineProperty(tools,metadata.name,{
  enumerable:true,value:(...args)=>native('tool',String(index),args.length?JSON.stringify(args[0]):undefined)
 }));
 globalThis.text = value => native('text',typeof value === 'string'?value:String(JSON.stringify(value)));
 globalThis.store = (key,value) => native('store',String(key),JSON.stringify(value));
 globalThis.load = key => {const value=native('load',String(key));return value===undefined?undefined:JSON.parse(value)};
 globalThis.notify = value => native('notify',typeof value==='string'?value:String(JSON.stringify(value)),callId);
 globalThis.setTimeout = (callback,millis=0) => {if(typeof callback!=='function')throw new TypeError('setTimeout requires a function');return native('timer',callback,Number(millis))};
 globalThis.clearTimeout = id => native('clear_timer',String(id));
 globalThis.exit = () => {native('exit');throw new Error('execution exited')};
 globalThis.yield_control = () => native('yield');
 for(const name of ['image','audio','generatedImage'])globalThis[name]=()=>{throw new Error(name+' output is not supported by the QuickJS backend yet')};
})
"#;
