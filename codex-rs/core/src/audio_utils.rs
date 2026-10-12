//! Audio input preparation is optional so text-only builds avoid the decoder stack.

#[cfg(feature = "voice")]
pub(crate) use codex_utils_audio::estimate_audio_token_count;
#[cfg(feature = "voice")]
pub(crate) use codex_utils_audio::prepare_response_items as prepare_audio_response_items;

#[cfg(not(feature = "voice"))]
pub(crate) fn estimate_audio_token_count(_audio_url: &str) -> usize {
    // Audio inputs are rejected before turn processing in text-only builds.
    // Historical audio is not decoded or charged by its encoded payload size.
    0
}

#[cfg(not(feature = "voice"))]
pub(crate) fn prepare_audio_response_items(items: &mut [codex_protocol::models::ResponseItem]) {
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::FunctionCallOutputContentItem;
    use codex_protocol::models::ResponseItem;

    const OMITTED: &str = "audio content omitted because voice support is disabled";

    for item in items {
        match item {
            ResponseItem::Message { content, .. } => {
                for content_item in content {
                    if matches!(content_item, ContentItem::InputAudio { .. }) {
                        *content_item = ContentItem::InputText {
                            text: OMITTED.to_string(),
                        };
                    }
                }
            }
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => {
                if let Some(content) = output.content_items_mut() {
                    for content_item in content {
                        if matches!(
                            content_item,
                            FunctionCallOutputContentItem::InputAudio { .. }
                        ) {
                            *content_item = FunctionCallOutputContentItem::InputText {
                                text: OMITTED.to_string(),
                            };
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
