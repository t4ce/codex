//! Present images selected in a completed live reply through the native viewer.

use super::*;
use crate::native_image_presentation::ImagePresentationResult;
use crate::native_image_presentation::present_images;

impl ChatWidget {
    pub(super) fn present_native_reply_images(
        &mut self,
        markdown: &str,
        turn_id: &str,
        item_id: &str,
        from_replay: bool,
    ) {
        if self.app_event_tx.voice_only.load(Ordering::Relaxed) {
            return;
        }
        let Some(thread_id) = self.thread_id else {
            return;
        };
        let sources = self.transcript.native_images.take_images(
            &thread_id.to_string(),
            turn_id,
            item_id,
            markdown,
            self.config.cwd.as_path(),
            from_replay,
        );
        if sources.is_empty() {
            return;
        }
        // The viewer has its own app identity and cannot read Codex's keyed
        // private directory. Export only the images selected in this reply.
        let presentation_root = PathBuf::from("/apps/common/codex");
        let factory = self.config.http_client_factory();
        let tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let results = present_images(sources, presentation_root, factory).await;
            tx.send(AppEvent::NativeImagePresentationFinished { thread_id, results });
        });
    }

    pub(crate) fn on_native_image_presentation_finished(
        &mut self,
        results: Vec<ImagePresentationResult>,
    ) {
        for result in results {
            if let Some(error) = result.error {
                let saved = result
                    .path
                    .map(|path| format!(" Saved at {}.", path.display()))
                    .unwrap_or_default();
                self.add_error_message(format!(
                    "Could not present image in img ({}): {error}.{saved}",
                    result.source,
                ));
            } else if let Some(path) = result.path {
                self.add_info_message(format!("Image sent to img: {}", path.display()), None);
            }
        }
    }
}
