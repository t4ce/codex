//! Send live image completions to the native viewer.

use super::*;
use crate::native_image_presentation::ImagePresentationResult;
use crate::native_image_presentation::ImageSource;
use crate::native_image_presentation::markdown_image_sources;
use crate::native_image_presentation::present_images;

impl ChatWidget {
    pub(super) fn present_native_reply_images(
        &mut self,
        markdown: &str,
        turn_id: &str,
        item_id: &str,
        from_replay: bool,
    ) {
        self.present_native_images(turn_id, item_id, from_replay, |cwd| {
            markdown_image_sources(markdown, cwd)
        });
    }

    pub(super) fn present_native_generated_image(
        &mut self,
        path: &AbsolutePathBuf,
        turn_id: &str,
        item_id: &str,
        from_replay: bool,
    ) {
        // A remote app server's saved path refers to its own filesystem.
        if self.snapshot_local_images {
            return;
        }
        self.present_native_images(turn_id, item_id, from_replay, |_| {
            vec![ImageSource::GeneratedFile(path.as_path().to_owned())]
        });
    }

    fn present_native_images(
        &mut self,
        turn_id: &str,
        item_id: &str,
        from_replay: bool,
        sources: impl FnOnce(&Path) -> Vec<ImageSource>,
    ) {
        if self.app_event_tx.voice_only.load(Ordering::Relaxed) {
            return;
        }
        let Some(thread_id) = self.thread_id else {
            return;
        };
        if !self.transcript.native_images.is_new_live_completion(
            &thread_id.to_string(),
            turn_id,
            item_id,
            from_replay,
        ) {
            return;
        }
        let sources = sources(self.config.cwd.as_path());
        if sources.is_empty() {
            return;
        }
        // The viewer has its own app identity and cannot read Codex's keyed
        // private directory. Give it copies in the shared directory.
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
