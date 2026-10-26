use super::*;

fn user(id: &str, text: &str) -> ThreadItem {
    ThreadItem::UserMessage {
        id: id.into(),
        client_id: None,
        content: vec![UserInput::Text {
            text: text.into(),
            text_elements: vec![],
        }],
    }
}

fn assistant(id: &str, text: &str, phase: MessagePhase) -> ThreadItem {
    ThreadItem::AgentMessage {
        id: id.into(),
        text: text.into(),
        phase: Some(phase),
        memory_citation: None,
        delivery: None,
        questions: None,
    }
}

#[test]
fn filters_invalid_suggestions_and_allows_abstention() {
    assert_eq!(
        parse_suggestion(r#"{"prompt":" Add tests  for Ubuntu\nplease "}"#),
        Some("Add tests for Ubuntu please".into())
    );
    for response in [
        r#"{"prompt":null}"#,
        r#"{"prompt":" "}"#,
        r#"{"prompt":"/quit"}"#,
        r#"{"prompt":"!rm something"}"#,
        r#"{"prompt":"bad\u001b[0m"}"#,
        "plain text",
        r#"{"prompt":"Fine","extra":true}"#,
    ] {
        assert_eq!(parse_suggestion(response), None, "{response}");
    }
    assert!(
        parse_suggestion(&json!({"prompt": "x".repeat(MAX_PROMPT_CHARS + 1)}).to_string())
            .is_none()
    );
}

#[test]
fn prompt_keeps_recent_intent_ignores_commentary_and_bounds_unicode_context() {
    let mut items = vec![user("old", "Obsolete task")];
    for index in 0..RECENT_MESSAGES {
        items.push(user(
            &format!("user-{index}"),
            &format!("Fix TRUEOS {}", "🚀".repeat(MESSAGE_BYTES)),
        ));
        items.push(assistant(
            &format!("answer-{index}"),
            "Ubuntu needs the same fix",
            MessagePhase::FinalAnswer,
        ));
    }
    items.push(assistant(
        "commentary",
        "PRIVATE COMMENTARY",
        MessagePhase::Commentary,
    ));
    let prompt = suggestion_prompt(&items).unwrap();
    assert!(!prompt.contains("Obsolete task"));
    assert!(!prompt.contains("PRIVATE COMMENTARY"));
    let messages: serde_json::Value =
        serde_json::from_str(prompt.split_once("\n\n").unwrap().1).unwrap();
    assert_eq!(messages.as_array().unwrap().len(), RECENT_MESSAGES);
    assert!(prompt.contains("Fix TRUEOS"));
    assert!(prompt.len() < RECENT_MESSAGES * MESSAGE_BYTES + 2_000);
}

#[test]
fn needs_a_user_and_a_completed_assistant_response() {
    assert!(suggestion_prompt(&[user("u", "Fix it")]).is_none());
    assert!(suggestion_prompt(&[assistant("a", "Done", MessagePhase::FinalAnswer)]).is_none());
    assert!(
        suggestion_prompt(&[
            user("u", "Fix it"),
            assistant("a", "Done", MessagePhase::FinalAnswer)
        ])
        .is_some()
    );
}

#[tokio::test]
async fn live_completion_generates_a_tool_free_editable_suggestion() -> color_eyre::Result<()> {
    use crate::app::session_lifecycle::ThreadAttachPresentation;
    use crate::app::test_support::make_test_app;
    use crate::app_event_sender::AppEventSender;
    use crate::chatwidget::tests::helpers::render_bottom_popup;
    use crate::test_support::PathBufExt;
    use codex_app_server_client::AppServerEvent;
    use codex_app_server_protocol::Turn;
    use codex_app_server_protocol::TurnCompletedNotification;
    use codex_app_server_protocol::TurnStatus;
    use codex_model_provider_info::ModelProviderInfo;
    use core_test_support::responses;
    use crossterm::event::KeyCode;

    let server = wiremock::MockServer::start().await;
    let response = responses::mount_response_once(&server, responses::sse_response(responses::sse(vec![
        responses::ev_response_created("suggestion-response"),
        responses::ev_assistant_message("suggestion-message", r#"{"prompt":"Add that feature for Ubuntu and TRUEOS"}"#),
        responses::ev_completed("suggestion-response"),
    ]))).await;
    let home = tempfile::tempdir()?;
    let provider_id = "next-prompt-test";
    std::fs::write(home.path().join("config.toml"), format!(
        "model = \"gpt-5.2\"\nmodel_provider = \"{provider_id}\"\n\
         [model_providers.{provider_id}]\nname = \"Next prompt test\"\n\
         base_url = \"{}/v1\"\nwire_api = \"responses\"\n\
         request_max_retries = 0\nstream_max_retries = 0\n", server.uri()
    ))?;
    let mut app = make_test_app().await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    app.local_settings.tui.animations = false;
    app.app_event_tx = AppEventSender::new(event_tx);
    app.config.codex_home = home.path().to_path_buf().abs();
    app.config.sqlite = codex_state::SqliteConfig::new_for_testing(home.path().to_path_buf().abs());
    app.config.model = Some("gpt-5.2".into());
    app.config.model_provider_id = provider_id.into();
    app.config.model_provider = ModelProviderInfo {
        name: "Next prompt test".into(),
        base_url: Some(format!("{}/v1", server.uri())),
        request_max_retries: Some(0), stream_max_retries: Some(0),
        ..ModelProviderInfo::default()
    };
    let mut tui = crate::tui::test_support::make_test_tui()?;
    let mut app_server = crate::start_embedded_app_server_for_picker(&app.config).await?;
    let started = app_server.start_thread(&app.config).await?;
    let thread_id = started.session.thread_id;
    app.replace_chat_widget_with_app_server_thread(&mut tui, started,
        ThreadAttachPresentation::SessionLineage, None).await?;
    let turn = Turn {
        id: "completed-turn".into(), root_turn_id: None,
        items: vec![user("u", "Does the CLI have next prompt suggestions?"),
            assistant("a", "The shared CLI does not implement them yet.", MessagePhase::FinalAnswer)],
        items_view: Default::default(), status: TurnStatus::Completed,
        error: None, started_at: None, completed_at: None, duration_ms: None,
    };
    app.ensure_thread_channel(thread_id).store.lock().await.turns.push(turn.clone());
    while event_rx.try_recv().is_ok() {}
    app.handle_active_thread_event(&mut tui, &mut app_server,
        ThreadBufferedEvent::Notification(Box::new(ServerNotification::TurnCompleted(
            TurnCompletedNotification { thread_id: thread_id.to_string(), turn }
        )))).await?;

    let mut saw_request = false;
    let mut saw_started = false;
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            tokio::select! {
                Some(event) = event_rx.recv() => {
                    saw_request |= matches!(&event, AppEvent::SuggestNextPrompt { .. });
                    saw_started |= matches!(&event, AppEvent::NextPromptStarted { .. });
                    let generated = matches!(&event, AppEvent::GeneratedNextPrompt { .. });
                    app.handle_event(&mut tui, &mut app_server, event).await?;
                    if generated { break; }
                }
                Some(event) = app_server.next_event() => {
                    if let AppServerEvent::ServerNotification(notification) = &event
                        && let ServerNotification::ThreadStarted(started) = notification.as_ref()
                        && started.thread.ephemeral
                    {
                        assert_eq!(started.thread.thread_source, Some(ThreadSource::Feature("next_prompt_suggestion".into())));
                    }
                    app.handle_app_server_event(&app_server, event).await;
                }
            }
        }
        color_eyre::Result::<()>::Ok(())
    }).await??;
    assert!(saw_request && saw_started);
    assert!(app.chat_widget.composer_is_empty());
    assert!(render_bottom_popup(&app.chat_widget, 120).contains("Add that feature for Ubuntu and TRUEOS"));
    app.chat_widget.handle_key_event(KeyCode::Tab.into());
    assert!(!app.chat_widget.composer_is_empty());
    assert!(app.pending_next_prompt.is_none());
    assert!(app.temporary_structured_requests.is_empty());
    let request = response.single_request().body_json();
    assert!(request["tools"].as_array().is_none_or(Vec::is_empty));
    assert!(request.to_string().contains("Does the CLI have next prompt suggestions?"));
    assert!(request.to_string().contains("The shared CLI does not implement them yet."));
    app_server.shutdown().await?;
    Ok(())
}
