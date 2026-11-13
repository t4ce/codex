use super::*;
use crate::native_image_presentation::ImagePresentationResult;

#[tokio::test]
async fn native_image_presentation_live_completion_queues_once_and_reports_failure() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let markdown = "![Manhattan map](missing-manhattan-map.png)";
    chat.present_native_reply_images(markdown, "turn", "reply", false);
    chat.present_native_reply_images(markdown, "turn", "reply", false);
    let results = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(AppEvent::NativeImagePresentationFinished {
                thread_id: owner,
                results,
            }) = rx.recv().await
            {
                assert_eq!(owner, thread_id);
                break results;
            }
        }
    })
    .await
    .expect("background presentation finishes");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].source, "missing-manhattan-map.png");
    assert!(results[0].error.is_some());
    assert!(
        !std::iter::from_fn(|| rx.try_recv().ok())
            .any(|event| { matches!(event, AppEvent::NativeImagePresentationFinished { .. }) })
    );
    chat.on_native_image_presentation_finished(results);
    let output = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(output.contains("Could not present image in img"));
    assert!(output.contains("missing-manhattan-map.png"));
}

#[tokio::test]
async fn native_image_presentation_replay_and_parked_voice_do_not_open_windows() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(None).await;
    chat.thread_id = Some(ThreadId::new());
    let markdown = "![Manhattan map](missing-manhattan-map.png)";
    chat.present_native_reply_images(markdown, "turn", "history", true);
    chat.present_native_reply_images(markdown, "turn", "history", false);
    chat.app_event_tx.voice_only.store(true, Ordering::Relaxed);
    chat.present_native_reply_images(markdown, "turn", "voice", false);
    tokio::task::yield_now().await;
    assert!(
        !std::iter::from_fn(|| rx.try_recv().ok())
            .any(|event| { matches!(event, AppEvent::NativeImagePresentationFinished { .. }) })
    );
}

#[tokio::test]
async fn native_image_presentation_status_distinguishes_admission_from_failure() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(None).await;
    let path = PathBuf::from("/apps/common/codex/presented_images/map.png");
    chat.on_native_image_presentation_finished(vec![
        ImagePresentationResult {
            source: "map".into(),
            path: Some(path.clone()),
            error: None,
        },
        ImagePresentationResult {
            source: "other map".into(),
            path: Some(path.clone()),
            error: Some("img admission failed (-3)".into()),
        },
    ]);
    let output = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(output.contains("Image sent to img:"));
    assert!(output.contains("Could not present image in img (other map):"));
    assert!(output.contains(&format!("Saved at {}", path.display())));
    assert!(!output.contains("displayed"));
}
