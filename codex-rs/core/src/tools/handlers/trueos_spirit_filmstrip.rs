//! Sample one shared UI4 window and compose an in-memory PNG for model input.

use std::time::Duration;

use image::ImageEncoder;
use image::ImageFormat;
use image::RgbImage;
use image::codecs::png::PngEncoder;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use tokio::time::Instant;
use tokio::time::sleep_until;

use crate::function_tool::FunctionCallError;

const MAX_WIDTH: u32 = 1920;
const MAX_HEIGHT: u32 = 1080;

pub(super) const fn default_filmstrip_frames() -> u32 {
    12
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FilmstripRequest {
    pub(super) duration_seconds: u32,
    #[serde(default = "default_filmstrip_frames")]
    pub(super) frames: u32,
}

#[derive(Deserialize)]
struct ObservationMetadata {
    id: String,
    publish_serial: u64,
}

impl FilmstripRequest {
    pub(super) fn validate(&self) -> Result<(), String> {
        if !(1..=10).contains(&self.duration_seconds) {
            return Err("filmstrip duration_seconds must be an integer from 1 to 10".into());
        }
        if !(8..=16).contains(&self.frames) {
            return Err("filmstrip frames must be an integer from 8 to 16".into());
        }
        Ok(())
    }

    pub(super) async fn capture(
        &self,
        mut observe: impl FnMut() -> Result<(String, Vec<u8>), FunctionCallError>,
    ) -> Result<(Value, Vec<u8>), FunctionCallError> {
        self.validate().map_err(FunctionCallError::RespondToModel)?;
        let start = Instant::now();
        let duration = Duration::from_secs(u64::from(self.duration_seconds));
        let deadline = |index| start + duration * index / (self.frames - 1);
        let mut window_id = None;
        let mut timestamps = Vec::new();
        let mut samples = Vec::new();
        for index in 0..self.frames {
            let now = Instant::now();
            if now >= start + duration {
                break;
            }
            if now < deadline(index) {
                sleep_until(deadline(index)).await;
            }
            // Drop elapsed slots instead of issuing a burst of captures after a stall.
            if index + 1 < self.frames && Instant::now() >= deadline(index + 1) {
                continue;
            }
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let (metadata, png) = observe()?;
            let metadata: ObservationMetadata =
                serde_json::from_str(&metadata).map_err(|error| {
                    FunctionCallError::RespondToModel(format!(
                        "invalid Spirit filmstrip observation metadata: {error}"
                    ))
                })?;
            if window_id.get_or_insert(metadata.id.clone()) != &metadata.id {
                return Err(FunctionCallError::RespondToModel(
                    "Spirit's selected window changed during filmstrip capture; select the target and retry"
                        .into(),
                ));
            }
            timestamps.push(json!({
                "elapsed_ms": elapsed_ms,
                "publish_serial": metadata.publish_serial,
            }));
            samples.push(png);
        }
        if samples.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "no Spirit filmstrip samples were captured".into(),
            ));
        }
        let (columns, rows) = layout(samples.len() as u32);
        let png = tokio::task::spawn_blocking(move || encode_filmstrip(samples, columns, rows))
            .await
            .map_err(|error| {
                FunctionCallError::RespondToModel(format!(
                    "Spirit filmstrip worker failed: {error}"
                ))
            })?
            .map_err(|error| {
                FunctionCallError::RespondToModel(format!(
                    "could not encode Spirit filmstrip: {error}"
                ))
            })?;
        Ok((
            json!({
                "window_id": window_id,
                "duration_seconds": self.duration_seconds,
                "requested_frames": self.frames,
                "columns": columns,
                "rows": rows,
                "tile_order": "left to right, then top to bottom",
                "samples": timestamps,
            }),
            png,
        ))
    }
}

fn layout(count: u32) -> (u32, u32) {
    let columns = if count <= 9 { count.min(3) } else { 4 };
    (columns, count.div_ceil(columns))
}

fn encode_filmstrip(samples: Vec<Vec<u8>>, columns: u32, rows: u32) -> image::ImageResult<Vec<u8>> {
    let cell_width = MAX_WIDTH / columns;
    let cell_height = MAX_HEIGHT / rows;
    let mut canvas = RgbImage::from_pixel(
        cell_width * columns,
        cell_height * rows,
        image::Rgb([24, 24, 28]),
    );
    for (index, png) in samples.into_iter().enumerate() {
        let frame = image::load_from_memory_with_format(&png, ImageFormat::Png)?;
        // Observations can already be smaller than a cell. Preserve those
        // pixels and letterbox rather than interpolating a larger image.
        let frame = if frame.width() > cell_width || frame.height() > cell_height {
            frame.resize(
                cell_width,
                cell_height,
                image::imageops::FilterType::Triangle,
            )
        } else {
            frame
        };
        let frame = frame.into_rgb8();
        let column = index as u32 % columns;
        let row = index as u32 / columns;
        let x = column * cell_width + (cell_width - frame.width()) / 2;
        let y = row * cell_height + (cell_height - frame.height()) / 2;
        image::imageops::replace(&mut canvas, &frame, i64::from(x), i64::from(y));
    }
    let mut png = Vec::new();
    PngEncoder::new(&mut png).write_image(
        canvas.as_raw(),
        canvas.width(),
        canvas.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, serial: u64, red: u8) -> (String, Vec<u8>) {
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&[red, 0, 0], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        (json!({"id": id, "publish_serial": serial}).to_string(), png)
    }

    #[tokio::test(start_paused = true)]
    async fn filmstrip_places_samples_in_order_and_spans_the_requested_duration() {
        let request: FilmstripRequest =
            serde_json::from_value(json!({"duration_seconds": 3})).unwrap();
        let mut serial = 0;
        let (metadata, png) = request
            .capture(|| {
                serial += 1;
                Ok(sample("42", serial, serial as u8))
            })
            .await
            .unwrap();
        assert_eq!(metadata["window_id"], "42");
        assert_eq!(metadata["columns"], 4);
        assert_eq!(metadata["rows"], 3);
        assert_eq!(metadata["samples"].as_array().unwrap().len(), 12);
        assert_eq!(metadata["samples"][0]["elapsed_ms"], 0);
        let last_ms = metadata["samples"][11]["elapsed_ms"].as_u64().unwrap();
        assert!((3000..=3001).contains(&last_ms));
        assert_eq!(metadata["samples"][11]["publish_serial"], 12);
        let canvas = image::load_from_memory(&png).unwrap().into_rgb8();
        assert_eq!(canvas.dimensions(), (1920, 1080));
        assert_eq!(canvas.get_pixel(239, 179).0, [1, 0, 0]);
        assert_eq!(canvas.get_pixel(1679, 899).0, [12, 0, 0]);
    }

    #[tokio::test(start_paused = true)]
    async fn filmstrip_aborts_on_selection_change_or_capture_failure() {
        let request = FilmstripRequest {
            duration_seconds: 1,
            frames: 8,
        };
        let mut serial = 0;
        let error = request
            .capture(|| {
                serial += 1;
                Ok(sample(if serial == 1 { "42" } else { "43" }, serial, 0))
            })
            .await
            .unwrap_err();
        assert!(
            matches!(error, FunctionCallError::RespondToModel(message) if message.contains("selected window changed"))
        );
        assert_eq!(serial, 2);
        assert!(
            request
                .capture(|| Err(FunctionCallError::RespondToModel("revoked".into())))
                .await
                .is_err()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn delayed_sampler_drops_missed_slots() {
        let request = FilmstripRequest {
            duration_seconds: 1,
            frames: 8,
        };
        let capture = request.capture(|| Ok(sample("42", 1, 0)));
        tokio::pin!(capture);
        assert!(futures::poll!(&mut capture).is_pending());
        tokio::time::advance(Duration::from_millis(750)).await;
        let (metadata, _) = capture.await.unwrap();
        let samples = metadata["samples"].as_array().unwrap();
        assert_eq!(samples.len(), 4);
        assert_eq!(samples[0]["elapsed_ms"], 0);
        assert_eq!(samples[1]["elapsed_ms"], 750);
        assert!(samples[2]["elapsed_ms"].as_u64().unwrap() >= 857);
    }

    #[test]
    fn request_limits_allow_detail_and_temporal_layouts() {
        for (duration_seconds, frames) in [(1, 8), (10, 16)] {
            assert!(
                FilmstripRequest {
                    duration_seconds,
                    frames
                }
                .validate()
                .is_ok()
            );
        }
        for (duration_seconds, frames) in [(0, 8), (11, 8), (1, 7), (1, 17)] {
            assert!(
                FilmstripRequest {
                    duration_seconds,
                    frames
                }
                .validate()
                .is_err()
            );
        }
        assert_eq!(layout(8), (3, 3));
        assert_eq!(layout(16), (4, 4));
    }
}
